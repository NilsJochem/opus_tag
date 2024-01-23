#![allow(dead_code)]
use itertools::Itertools;
use std::{
    fmt::Debug,
    io::{self, Read, Write},
    path::Path,
};
use thiserror::Error;

use crate::{
    error::{self, Error},
    require,
};

const MAGIC_STR: &[u8] = b"OggS";
const OGG_CRC: crc::Crc<u32> = crc::Crc::<u32>::new(&crc::Algorithm {
    width: 32,
    poly: 0x04C1_1DB7,
    init: 0x0000_0000,
    refin: false,
    refout: false,
    xorout: 0x0000_0000,
    check: 0x0000_0000,
    residue: 0x0000_0000,
});

#[allow(clippy::module_name_repetitions)]
#[derive(PartialEq, Eq)]
pub struct OggPage {
    pub header_type: HeaderType,
    pub granule_position: u64,
    pub bitstream_serial_number: u32,
    pub page_sequence_number: u32,
    /// invariant, len (and sublen) are bound to u8::MAX
    segment_table: Vec<Vec<u8>>,
}

impl Debug for OggPage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OggPage")
            .field("header_type", &self.header_type)
            .field("granule_position", &self.granule_position)
            .field("bitstream_serial_number", &self.bitstream_serial_number)
            .field("page_sequence_number", &self.page_sequence_number)
            .field(
                "page_segments",
                &self.segment_table.iter().map(Vec::len).collect_vec(),
            )
            .finish()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum HeaderType {
    Simple,
    Continuation,
    BoS,
    EoS,
}

impl TryFrom<u8> for HeaderType {
    type Error = u8;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x00 => Ok(Self::Simple),
            0x01 => Ok(Self::Continuation),
            0x02 => Ok(Self::BoS),
            0x04 => Ok(Self::EoS),
            value => Err(value),
        }
    }
}
impl From<HeaderType> for u8 {
    fn from(value: HeaderType) -> Self {
        match value {
            HeaderType::Simple => 0x00,
            HeaderType::Continuation => 0x01,
            HeaderType::BoS => 0x02,
            HeaderType::EoS => 0x04,
        }
    }
}

#[derive(Debug, Error)]
pub enum SegmentToLarge {
    #[error("{size} segment[{position:?}] to long, only {} allowed", u8::MAX)]
    SegmentToLong { size: usize, position: usize },
    #[error("{size} are to many segments, only {} allowed", u8::MAX)]
    TooManySegments { size: usize },
}
impl SegmentToLarge {
    fn validate(data: &Vec<Vec<u8>>) -> Result<(), Self> {
        require!(
            u8::try_from(data.len()).is_ok(),
            Self::TooManySegments { size: data.len() }
        );
        for (i, segment) in data.iter().enumerate() {
            require!(
                u8::try_from(segment.len()).is_ok(),
                Self::SegmentToLong {
                    size: segment.len(),
                    position: i,
                }
            );
        }
        Ok(())
    }
    fn validate_new(data: &Vec<Vec<u8>>, new: &Vec<u8>) -> Result<(), Self> {
        require!(
            u8::try_from(data.len() + 1).is_ok(),
            Self::TooManySegments {
                size: data.len() + 1,
            }
        );
        require!(
            u8::try_from(new.len()).is_ok(),
            Self::SegmentToLong {
                size: new.len(),
                position: data.len(),
            }
        );
        Ok(())
    }
}
impl OggPage {
    pub fn new(
        header_type: HeaderType,
        granule_position: u64,
        bitstream_serial_number: u32,
        page_sequence_number: u32,
        segment_table: Vec<Vec<u8>>,
    ) -> Result<Self, SegmentToLarge> {
        SegmentToLarge::validate(&segment_table)?;
        Ok(Self {
            header_type,
            granule_position,
            bitstream_serial_number,
            page_sequence_number,
            segment_table,
        })
    }
    pub const fn segment_table(&self) -> &Vec<Vec<u8>> {
        &self.segment_table
    }
    pub fn set_segment_table(&mut self, segment_table: Vec<Vec<u8>>) -> Result<(), SegmentToLarge> {
        SegmentToLarge::validate(&segment_table)?;
        self.segment_table = segment_table;
        Ok(())
    }
    pub fn add_segment(&mut self, segment: Vec<u8>) -> Result<(), SegmentToLarge> {
        SegmentToLarge::validate_new(&self.segment_table, &segment)?;
        self.segment_table.push(segment);
        Ok(())
    }

    pub fn write_to(self, writer: &mut impl Write) -> Result<(), io::Error> {
        let mut buf = Vec::new();
        // the exact size is known, so this is prefered over Vec::with_capacity
        buf.reserve_exact(
            27 + self.segment_table.len() + self.segment_table.iter().map(Vec::len).sum::<usize>(),
        );

        buf.extend(MAGIC_STR);
        buf.push(0);
        buf.push(self.header_type.into());
        buf.extend(&self.granule_position.to_le_bytes());
        buf.extend(&self.bitstream_serial_number.to_le_bytes());
        buf.extend(&self.page_sequence_number.to_le_bytes());
        buf.extend([0; 4]);
        // invariant uphold on construction
        buf.push(self.segment_table.len() as u8);
        buf.extend(self.segment_table.iter().map(|it| it.len() as u8));
        buf.extend(self.segment_table.iter().flatten());

        Self::calculate_checksum(&mut buf);
        writer.write_all(&buf)
    }

    /// [spec](https://en.wikipedia.org/wiki/Ogg#Page_structure)
    pub fn read_next_from<R: Read>(data: &mut R) -> Result<Self, error::Error> {
        let mut buf = vec![0; 27];
        read_exact(data, &mut buf)?;

        error::Error::expect_starts_with(&buf, MAGIC_STR)?;
        let page_segments = buf[26];
        let mut segment_sizes = vec![0; page_segments as usize];
        data.read_exact(&mut segment_sizes)?;
        let segment_table = segment_sizes
            .iter()
            .map(|size| {
                let mut segment = vec![0; *size as usize];
                data.read_exact(&mut segment).map(|_| segment)
            })
            .collect::<Result<Vec<_>, _>>()?;

        // add all data that was read to one buffer to perform checksum
        buf.extend(segment_sizes.iter().chain(segment_table.iter().flatten()));

        require!(
            Self::validate_checksum(&mut buf),
            error::Error::MalformedData("checksum wrong".to_owned())
        );

        let version = buf[4];
        assert_eq!(version, 0, "version is mandated to be zero");
        Ok(Self {
            header_type: buf[5]
                .try_into()
                .map_err(|err| error::Error::MalformedData(format!("unkown header_type {err}")))?,
            granule_position: u64::from_le_bytes(buf[6..14].try_into().unwrap()),
            bitstream_serial_number: u32::from_le_bytes(buf[14..18].try_into().unwrap()),
            page_sequence_number: u32::from_le_bytes(buf[18..22].try_into().unwrap()),
            segment_table,
        })
    }

    pub fn iterate_read(mut data: impl Read) -> impl Iterator<Item = Result<Self, error::Error>> {
        let mut is_finished = false;
        std::iter::from_fn(move || {
            if is_finished {
                return None;
            }
            match Self::read_next_from(&mut data) {
                Err(err) => {
                    is_finished = true; // prevent more data from being read
                    match err {
                        Error::NoMoreData => None, // already ad EOF, can return None
                        _ => Some(Err(err)),
                    }
                }
                Ok(it) => Some(Ok(it)),
            }
        })
    }
    #[allow(dead_code)]
    pub fn iterate_file(
        path: impl AsRef<Path>,
    ) -> Result<impl Iterator<Item = Result<Self, error::Error>>, io::Error> {
        Ok(Self::iterate_read(std::fs::File::open(path)?))
    }

    /// # Side effect
    /// takes the checksum bytes (22..26) and leaves zeros
    fn validate_checksum(buf: &mut [u8]) -> bool {
        let mut check_bytes = [0; 4];
        check_bytes.swap_with_slice(&mut buf[22..26]);
        u32::from_le_bytes(check_bytes) == OGG_CRC.checksum(buf)
    }
    /// # Panics
    /// expects checksum bytes (22..26) to be zero and will panic otherwise
    /// # Side effect
    /// puts the checksum into its location
    fn calculate_checksum(buf: &mut [u8]) {
        assert_eq!([0; 4], buf[22..26], "checksum bytes need to be zero");
        let mut check_bytes = OGG_CRC.checksum(buf).to_le_bytes();
        check_bytes.swap_with_slice(&mut buf[22..26]);
    }
}

/// copy of [`Read::read_exact`], that reports [`Error::NoMoreData`] if nothing was read
fn read_exact(read: &mut impl Read, mut buf: &mut [u8]) -> Result<(), Error> {
    let mut starts_at_eof = !buf.is_empty(); // will not detect EOF with zero_read
    while !buf.is_empty() {
        match read.read(buf) {
            Ok(0) => break,
            Ok(n) => {
                starts_at_eof = false; // as soon as something is read cant start at EOF
                let tmp = buf;
                buf = &mut tmp[n..];
            }
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
    if starts_at_eof {
        Err(Error::NoMoreData)
    } else if !buf.is_empty() {
        Err(Error::UnexpectedEoF)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_FILE: &str = "./res/local/tag_test_small.opus";
    const END_PACKET_1: usize = 0x2F;
    const END_PACKET_2: usize = 0x1C9;
    const END_PACKET_3: usize = 0x9A3;

    const NUMBER_OGG_PACKETS: usize = 4660;

    #[test]
    fn read_write_equals() {
        let mut data_src = std::fs::File::open(TEST_FILE).unwrap();
        let mut buf = vec![0; END_PACKET_3];
        data_src.read_exact(&mut buf).unwrap();

        // there is a valid Ogg Packet at the start of the file with length 0x2F
        let header_data = &buf[..END_PACKET_1];
        // there is a valid Ogg Packet between 0x2F and 0x1C9
        let tags_data = &buf[END_PACKET_1..END_PACKET_2];
        // there is a valid Ogg Packet between 0x1C9 and the end of read bytes
        let first_audio = &buf[END_PACKET_2..];

        for data in [header_data, tags_data, first_audio] {
            let mut read_data = data;
            let head = OggPage::read_next_from(&mut read_data).unwrap();
            let mut out_data = Vec::new();
            head.write_to(&mut out_data).unwrap();
            assert_eq!(data.len(), out_data.len());
            assert_eq!(data, out_data);
        }
    }

    #[test]
    fn read_iter() {
        let data_src = std::fs::File::open(TEST_FILE).unwrap();

        // take only 0x1150, which is the size of the first 3 valid packets
        let oggs = OggPage::iterate_read(data_src.take(END_PACKET_3 as u64))
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(3, oggs.len(), "failed to read all 3 packets in data");
    }

    #[test]
    fn read_full_file() {
        let data_src = std::fs::File::open(TEST_FILE).unwrap();

        let oggs = OggPage::iterate_read(data_src)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            NUMBER_OGG_PACKETS,
            oggs.len(),
            "failed to read all 1987 packets in data"
        );
    }
}
