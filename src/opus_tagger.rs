use std::{
    fmt::Debug,
    io::{Read, Write},
    path::Path,
};

use crate::{
    error::{self, Error},
    ogg::OggPage,
    require, MultiChain,
};
use itertools::Itertools;

const HEAD_MAGIC_STR: &[u8] = b"OpusHead";
const HEAD_VERSION: u8 = 1;
#[derive(Debug, PartialEq, Eq)]
pub struct OpusHead {
    version: u8,
    channel_count: u8,
    pre_skip: u16,
    sample_rate: SampleRate,
    gain: Gain,
    channel_map: MappingFamily,
}
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum MappingFamily {
    RTP,
    VorbisChannelOrder,
    NotDefined(u8),
}
impl From<u8> for MappingFamily {
    fn from(value: u8) -> Self {
        match value {
            0 => Self::RTP,
            1 => Self::VorbisChannelOrder,
            value => Self::NotDefined(value),
        }
    }
}
impl From<MappingFamily> for u8 {
    fn from(value: MappingFamily) -> Self {
        match value {
            MappingFamily::RTP => 0,
            MappingFamily::VorbisChannelOrder => 1,
            MappingFamily::NotDefined(nr) => nr,
        }
    }
}
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum SampleRate {
    KHz8,
    KHz12,
    KHz16,
    KHz24,
    KHz48,
}
impl From<SampleRate> for u32 {
    fn from(value: SampleRate) -> Self {
        match value {
            SampleRate::KHz8 => 8000,
            SampleRate::KHz12 => 12000,
            SampleRate::KHz16 => 16000,
            SampleRate::KHz24 => 24000,
            SampleRate::KHz48 => 48000,
        }
    }
}
impl From<SampleRate> for [u8; 4] {
    fn from(value: SampleRate) -> Self {
        u32::from(value).to_le_bytes()
    }
}
impl TryFrom<u32> for SampleRate {
    type Error = u32;
    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            8000 => Ok(Self::KHz8),
            12000 => Ok(Self::KHz12),
            16000 => Ok(Self::KHz16),
            24000 => Ok(Self::KHz24),
            48000 => Ok(Self::KHz48),
            sr => Err(sr),
        }
    }
}
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
// a number in Q7.8 format
pub struct Gain {
    m: i8, // only 7 bits for M, first bit is sign
    n: u8,
}

impl OpusHead {
    #[allow(dead_code)]
    fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.reserve_exact(19);
        buf.extend(HEAD_MAGIC_STR);
        buf.push(HEAD_VERSION);
        buf.push(self.channel_count);
        buf.extend(self.pre_skip.to_le_bytes());
        buf.extend(<SampleRate as Into<[u8; 4]>>::into(self.sample_rate));
        buf.push(self.gain.m.to_le_bytes()[0]);
        buf.push(self.gain.n);
        buf.push(self.channel_map.into());

        // TODO Optional Channel Mapping
        buf
    }
    /// [spec](https://wiki.xiph.org/OggOpus#ID_Header)
    fn from(ogg_head: &OggPage) -> Result<Self, error::Error> {
        assert_eq!(ogg_head.granule_position, 0, "granule needs to be zero");
        require!(
            ogg_head.segment_table().len() == 1,
            error::Error::MalformedData(format!(
                "expected one segment, got mutliple with sizes: {:?}",
                ogg_head.segment_table().iter().map(Vec::len).collect_vec()
            ))
        );
        let buf = ogg_head.segment_table()[0].as_slice();
        require!(
            buf.len() == 19, // maybe can be 19..19+(channel*8)
            error::Error::MalformedData(format!(
                "OpusHead needs to be length 19, but was {}",
                buf.len(),
            ))
        );

        error::Error::expect_starts_with(buf, HEAD_MAGIC_STR)?;

        let version = buf[8];
        require!(version <= 15, error::Error::UnsupportetVersion(version));
        // TODO validate
        let channel_count = buf[9];
        let channel_map = buf[18].into();

        Ok(Self {
            version,
            channel_count,
            pre_skip: u16::from_le_bytes(buf[10..12].try_into().unwrap()),
            sample_rate: u32::from_le_bytes(buf[12..16].try_into().unwrap())
                .try_into()
                .unwrap(),
            gain: Gain {
                m: i8::from_le_bytes([buf[16]]),
                n: buf[17],
            },
            channel_map,
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct VorbisComment {
    vendor: String,
    comments: Vec<Comment>,
}
#[derive(Debug, PartialEq, Eq)]
pub struct Comment {
    pub key: String,
    pub value: String,
}
impl<IntoK: Into<String>, IntoV: Into<String>> From<(IntoK, IntoV)> for Comment {
    fn from(value: (IntoK, IntoV)) -> Self {
        Self {
            key: value.0.into(),
            value: value.1.into(),
        }
    }
}
impl VorbisComment {
    pub fn empty(vendor: impl Into<String>) -> Self {
        Self {
            vendor: vendor.into(),
            comments: Vec::new(),
        }
    }
    pub fn new<Iter: IntoIterator>(vendor: impl Into<String>, comments: Iter) -> Self
    where
        Iter::Item: Into<Comment>,
    {
        Self {
            vendor: vendor.into(),
            comments: comments
                .into_iter()
                .map(Into::<Comment>::into)
                .collect_vec(),
        }
    }
    pub fn add_comment(&mut self, comment: impl Into<Comment>) {
        self.comments.push(comment.into());
    }
    pub fn find_comments(&self, key: impl AsRef<str>) -> impl Iterator<Item = &Comment> {
        self.comments
            .iter()
            .filter(move |it| it.key.eq_ignore_ascii_case(key.as_ref()))
    }
    pub fn remove_first(&mut self, key: impl AsRef<str>) -> Option<Comment> {
        let element =
            self.comments.iter().enumerate().find_map(|(i, comment)| {
                (comment.key.eq_ignore_ascii_case(key.as_ref())).then_some(i)
            });
        element.map(|i| self.comments.remove(i))
    }
    pub fn remove_all(&mut self, key: impl AsRef<str>) {
        // todo return removed
        self.comments
            .retain(|it| !it.key.eq_ignore_ascii_case(key.as_ref()));
    }

    /// reads opus metadata from `from`, updates the [`OpusTags`] and writes the whole updated stream to `to`
    fn update_opus_tags(&self, mut from: impl Read, mut to: impl Write) -> Result<(), Error> {
        let mut iter = OggPage::iterate_read(&mut from);
        let head_ogg = iter
            .next()
            .ok_or_else(|| Error::MalformedData("missing first ogg_packet".to_owned()))??;
        let mut tags_ogg = iter
            .next()
            .ok_or_else(|| Error::MalformedData("missing second ogg_packet".to_owned()))??;
        drop(iter);

        // validate current data
        let _tags = Self::from(&tags_ogg, TAGS_MAGIC_STR)?;
        let _head = OpusHead::from(&head_ogg)?;

        let table = self
            .to_bytes(TAGS_MAGIC_STR)
            .chunks(255)
            .map(<[u8]>::to_vec)
            .collect_vec();
        tags_ogg.set_segment_table(table).unwrap();

        head_ogg.write_to(&mut to)?;
        tags_ogg.write_to(&mut to)?;

        std::io::copy(&mut from, &mut to)?;
        Ok(())
    }
    #[momo::momo]
    pub fn write_opus_file(&self, path: impl AsRef<Path>) -> Result<(), Error> {
        let file = std::fs::File::open(path).expect("file not found");
        let tmp_name = path.file_name().unwrap().to_string_lossy();
        let mut tmp_name =
            common::io::TmpFile::new_empty(path.with_file_name(format!(".{tmp_name}"))).unwrap();
        // .expect("tmp file already exists");
        let tmp_file = std::fs::File::options()
            .read(true)
            .write(true)
            .open(&tmp_name)
            .unwrap();

        self.update_opus_tags(file, tmp_file)?;

        std::fs::remove_file(path)?;
        std::fs::rename(&tmp_name, path).unwrap(); // this shouldn't fail, because then the file whill be lost
        tmp_name.was_removed(); // mark file to not autoremove

        Ok(())
    }

    fn to_bytes(&self, magic_str: &[u8]) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend(magic_str);
        write_length_encode_str(&mut buf, &self.vendor).unwrap();
        buf.extend((self.comments.len() as u32).to_le_bytes());
        for comment in &self.comments {
            write_length_encode_str(&mut buf, &format!("{}={}", comment.key, comment.value))
                .unwrap();
        }
        buf
    }
    /// [spec](https://wiki.xiph.org/OggOpus#Comment_Header)
    fn from(ogg_head: &OggPage, magic_str: &[u8]) -> Result<Self, error::Error> {
        assert_eq!(ogg_head.granule_position, 0, "granule needs to be zero");

        let all_seg_len = ogg_head.segment_table().iter().map(Vec::len).sum::<usize>();
        require!(
            all_seg_len >= 12,
            error::Error::MalformedData(format!(
                "comment packet needs to have a length of at least 12, but got {all_seg_len}"
            ))
        );
        let mut buf = MultiChain::new(ogg_head.segment_table().iter().map(std::vec::Vec::as_slice));

        error::Error::expect_starts_with_reader(&mut buf, magic_str)?;

        let vendor = read_length_encode_str(&mut buf)?;
        let number_tags = read_u32(&mut buf)?;

        let mut comments = Vec::with_capacity(number_tags as usize);
        for _ in 0..number_tags {
            let read = read_length_encode_str(&mut buf)?;
            let (key, value) = read.splitn(2, '=').collect_tuple().ok_or_else(|| {
                error::Error::MalformedData(format!("missing seperator '=' in {read:?}"))
            })?;
            comments.push((key, value).into());
        }
        Ok(Self { vendor, comments })
    }
}

const TAGS_MAGIC_STR: &[u8] = b"OpusTags";
#[derive(Debug, PartialEq, Eq)]
pub struct OpusMeta {
    pub head: OpusHead,
    pub tags: VorbisComment,
}
impl OpusMeta {
    /// reads `Self` from `path`
    ///
    /// # Errors
    /// when `data` doesn't start with a valid `OpusHead` and `VorbisComment`
    pub fn read_from<R: Read>(data: R) -> Result<Self, error::Error> {
        let mut iter = OggPage::iterate_read(data);
        let head = OpusHead::from(
            &iter
                .next()
                .ok_or_else(|| Error::MalformedData("missing first ogg_packet".to_owned()))??,
        )?;
        let tags = VorbisComment::from(
            &iter
                .next()
                .ok_or_else(|| Error::MalformedData("missing second ogg_packet".to_owned()))??,
            TAGS_MAGIC_STR,
        )?;
        Ok(Self { head, tags })
    }
    /// reads `Self` from `path`
    ///
    /// # Errors
    /// when the read errors
    /// when [`Self::read_from`] errors
    pub fn read_from_file(path: impl AsRef<Path>) -> Result<Self, error::Error> {
        let file = std::fs::File::open(path)?;
        Self::read_from(file)
    }
}

fn read_u32(read: &mut impl Read) -> Result<u32, error::Error> {
    let mut buf = [0; 4];
    read.read_exact(&mut buf)?;
    Ok(u32::from_le_bytes(buf))
}
fn read_length_encode_str(read: &mut impl Read) -> Result<String, error::Error> {
    let length = read_u32(read)?;
    let mut buf = vec![0; length as usize];

    read.read_exact(&mut buf)?;
    Ok(String::from_utf8(buf)?)
}
fn write_length_encode_str(write: &mut impl Write, s: &str) -> Result<(), error::Error> {
    let len: u32 = s.len().try_into().expect("string to long");
    write.write_all(&len.to_le_bytes())?;
    write.write_all(s.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_opus() {
        assert_eq!(
            OpusMeta {
                head: OpusHead {
                    version: 1,
                    channel_count: 2,
                    pre_skip: 312,
                    sample_rate: SampleRate::KHz48,
                    gain: Gain { m: 0, n: 0 },
                    channel_map: MappingFamily::RTP,
                },
                tags: VorbisComment::new(
                    "Lavf60.3.100",
                    vec![
                        ("TITLE", "Das Amulett der Mumie"),
                        ("ALBUM", "Gruselkabinett"),
                        ("GENRE", "H\u{f6}rbuch"),
                        ("TRACKNUMBER", "2"),
                        ("TOTALTRACKS", "182"),
                        ("ARTIST", "Bram Stoker "),
                        ("YEAR", "2004"),
                        ("CHAPTER000", "00:00:00.000"),
                        ("CHAPTER000NAME", "Part 1"),
                        ("CHAPTER001", "00:22:37.040"),
                        ("CHAPTER001NAME", "Part 2"),
                        ("CHAPTER002", "00:41:08.440"),
                        ("CHAPTER002NAME", "Part 3"),
                        ("CHAPTER003", "01:00:15.640"),
                        ("CHAPTER003NAME", "Part 4")
                    ],
                )
            },
            OpusMeta::read_from_file("./res/local/tag_test_small.opus").unwrap()
        );
    }

    #[test]
    fn test_read_big_opus() {
        assert_eq!(
            OpusMeta {
                head: OpusHead {
                    version: 1,
                    channel_count: 2,
                    pre_skip: 312,
                    sample_rate: SampleRate::KHz48,
                    gain: Gain { m: 0, n: 0 },
                    channel_map: MappingFamily::RTP,
                },
                tags: VorbisComment::new(
                    "Lavf60.3.100",
                    vec![
                        ("TITLE", "Das Amulett der Mumie 1 mit einer Menge extra um ein Ogg Page overflow zu provozieren, dazu weden insgesamt mindestens 255 Zeichen in allen tags ben\u{f6}tigt"),
                        ("ARTIST", "Bram Stoker"),
                        ("ALBUMARTIST", "alle felder sollten gef\u{fc}llt sein"),
                        ("ALBUM", "Gruselkabinett"),
                        ("DISCNUMBER", "02"),
                        ("DATE", "2004"),
                        ("TRACKNUMBER", "01"),
                        ("TRACKTOTAL", "00"),
                        ("GENRE", "H\u{f6}rbuch"),
                        ("DESCRIPTION", "alle felder sollten gef\u{fc}llt sein"),
                        ("COMPOSER", "alle felder sollten gef\u{fc}llt sein"),
                        ("PERFORMER", "alle felder sollten gef\u{fc}llt sein"),
                        ("COPYRIGHT", "alle felder sollten gef\u{fc}llt sein"),
                        ("CONTACT", "alle felder sollten gef\u{fc}llt sein"),
                        ("ENCODED-BY", "alle felder sollten gef\u{fc}llt sein")
                    ],
                )
            },
            OpusMeta::read_from_file("./res/local/tag_test_long.opus").unwrap()
        );
    }

    #[test]
    fn update_tags() {
        let mut data_src = std::fs::File::open("./res/local/tag_test_small.opus").unwrap();
        let mut buf = vec![0; 0x1150];
        data_src.read_exact(&mut buf).unwrap();

        let mut original_oggs = OggPage::iterate_read(buf.as_slice());

        let new_tags = VorbisComment::new(
            "something new",
            vec![
                ("TITLE", "erstmal weniger daten"),
                ("ARTIST", "ein paar felder sollten l\u{e4}nger werden"),
            ],
        );

        let mut new_buf = Vec::new();
        new_tags
            .update_opus_tags(buf.as_slice(), &mut new_buf)
            .unwrap();

        let mut new_oggs = OggPage::iterate_read(new_buf.as_slice());

        assert_eq!(
            original_oggs.next().unwrap().unwrap(),
            new_oggs.next().unwrap().unwrap(),
            "first Packet failed"
        );
        let _ = original_oggs.next().unwrap().unwrap();
        assert_eq!(
            new_tags,
            VorbisComment::from(&new_oggs.next().unwrap().unwrap(), TAGS_MAGIC_STR).unwrap(),
            "second Packet failed"
        );
        assert_eq!(
            original_oggs.next().unwrap().unwrap(),
            new_oggs.next().unwrap().unwrap(),
            "third Packet failed"
        );
    }
}
