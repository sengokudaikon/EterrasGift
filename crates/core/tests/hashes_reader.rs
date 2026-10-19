use std::io::{self, Read};

use eg_core::hashes;

#[test]
fn streaming_hash_matches_byte_hash_across_short_and_interrupted_reads() {
    struct Fragmented<'a> {
        remaining: &'a [u8],
        interrupt: bool,
    }
    impl Read for Fragmented<'_> {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            self.interrupt = !self.interrupt;
            if self.interrupt {
                return Err(io::Error::from(io::ErrorKind::Interrupted));
            }
            let count = self.remaining.len().min(output.len()).min(7);
            output[..count].copy_from_slice(&self.remaining[..count]);
            self.remaining = &self.remaining[count..];
            Ok(count)
        }
    }
    let bytes: Vec<_> = (0..200_000).map(|n| (n % 251) as u8).collect();
    assert_eq!(
        hashes::of_reader(Fragmented {
            remaining: &bytes,
            interrupt: false
        })
        .unwrap(),
        hashes::of_bytes(&bytes)
    );
    assert_eq!(hashes::of_reader(&[][..]).unwrap(), hashes::of_bytes(&[]));
}

#[test]
fn streaming_hash_propagates_input_errors_instead_of_returning_a_partial_digest() {
    struct Broken;
    impl Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::from(io::ErrorKind::UnexpectedEof))
        }
    }
    assert_eq!(hashes::of_reader(Broken).unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
}
