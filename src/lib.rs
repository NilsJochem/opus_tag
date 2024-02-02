#![warn(
    clippy::nursery,
    clippy::pedantic,
    clippy::empty_structs_with_brackets,
    clippy::format_push_string,
    clippy::if_then_some_else_none,
    clippy::impl_trait_in_params,
    clippy::missing_assert_message,
    clippy::multiple_inherent_impl,
    clippy::non_ascii_literal,
    clippy::self_named_module_files,
    clippy::semicolon_inside_block,
    clippy::separated_literal_suffix,
    clippy::str_to_string,
    clippy::string_to_string
)]
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_lossless,
    clippy::cast_sign_loss,
    clippy::single_match_else,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc
)]

use common::require;

pub mod error;
pub mod opus_tagger;

mod ogg;

impl error::Error {
    pub(crate) fn expect_starts_with(data: &[u8], expect: &[u8]) -> Result<(), Self> {
        let data = &data[0..expect.len()];
        require!(
            expect == data,
            Self::MalformedData(String::from_utf8(data.to_vec())
            .and_then(|data| String::from_utf8(expect.to_vec()).map(|it| (it, data)))
            .map_or_else(
                |_| {
                    format!(
                        "expected packet to start with MagicBytes {:?} but got {data:?}",
                        expect,
                    )
                },
                |(expect, data)| {
                    format!("expected packet to start with MagicString {expect:?} but got {data:?}")
                },
            ),)
        );
        Ok(())
    }
    pub(crate) fn expect_starts_with_reader(
        data: &mut impl std::io::Read,
        expect: &[u8],
    ) -> Result<(), Self> {
        let mut buf = vec![0; expect.len()];
        data.read_exact(&mut buf)?;
        Self::expect_starts_with(&buf, expect)
    }
}

struct MultiChain<Iter: Iterator> {
    iter: Iter,
    head: Option<Iter::Item>,
}
impl<Iter: Iterator> MultiChain<Iter> {
    fn new(iter: impl IntoIterator<IntoIter = Iter>) -> Self {
        Self {
            iter: iter.into_iter(),
            head: None,
        }
    }
}
impl<Iter: Iterator> std::io::Read for MultiChain<Iter>
where
    Iter::Item: std::io::Read,
{
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.head.is_none() {
            self.head = self.iter.next();
        }
        match self.head.as_mut() {
            Some(head) => {
                let result = head.read(buf);
                match result {
                    Ok(0) => {
                        self.head = None;
                        self.read(buf)
                    }
                    _ => result,
                }
            }
            None => Ok(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;

    #[test]
    fn multi_chain() {
        let raw_data = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
        let (mut a, mut b, mut c) = (&raw_data[0..3], &raw_data[3..7], &raw_data[7..10]);
        let mut data = MultiChain::new([&mut a, &mut b, &mut c]);

        let mut buf = [0; 10];
        data.read_exact(&mut buf).unwrap();

        assert_eq!(raw_data, buf);
    }
}
