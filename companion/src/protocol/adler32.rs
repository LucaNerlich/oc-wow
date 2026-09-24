//! Adler-32 checksum (RFC 1950) used by every OCWow transport packet.

const MOD_ADLER: u32 = 65521;

/// Compute the Adler-32 checksum of `data`.
pub fn adler32(data: &[u8]) -> u32 {
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for &byte in data {
        a = (a + byte as u32) % MOD_ADLER;
        b = (b + a) % MOD_ADLER;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_is_one() {
        assert_eq!(adler32(b""), 1);
    }

    #[test]
    fn wikipedia_vector() {
        // Well-known reference vector.
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn detects_single_bit_flip() {
        let a = adler32(b"hello world");
        let mut bytes = *b"hello world";
        bytes[0] ^= 0x01;
        assert_ne!(a, adler32(&bytes));
    }
}
