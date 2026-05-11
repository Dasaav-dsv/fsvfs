use std::{
    io::{self, BufRead, BufReader, Cursor, Write},
    iter, mem,
    num::NonZero,
};

use malachite::{Natural, base::num::arithmetic::traits::ModPow, platform::Limb};
use thiserror::Error;

#[derive(Clone, Debug)]
pub struct RsaKey {
    n: Natural,
    e: Natural,
    len: NonZero<usize>,
}

pub struct RsaDecryptor<'a, R> {
    key: &'a RsaKey,
    out: Cursor<Box<[u8]>>,
    reader: BufReader<R>,
}

#[derive(Debug, Error)]
pub enum DecodeError {
    #[error(transparent)]
    Base64(#[from] base64::DecodeError),

    #[error("malformed DER sequence or encoding")]
    Der,
}

#[derive(Debug, Error)]
pub enum DecryptError {
    #[error("input is not a multiple of block size ({0} bytes)")]
    Size(usize),

    #[error("input is not reduced mod `n` of the key")]
    Base,
}

const DER_INT: u8 = 0x02;
const DER_SEQ: u8 = 0x10 | 0x20;

impl RsaKey {
    #[inline(always)]
    pub fn in_block_len(&self) -> usize {
        self.len.get()
    }

    #[inline(always)]
    pub fn out_block_len(&self) -> usize {
        self.len.get() - 1
    }

    pub fn decode_from_pem(pem: &str) -> Result<Self, DecodeError> {
        let base64_content: String = pem
            .lines()
            .filter(|line| !line.starts_with("-----"))
            .collect();

        use base64::{Engine, engine::general_purpose::STANDARD};
        let der = STANDARD.decode(&base64_content)?;

        Self::decode_from_der(&der)
    }

    pub fn decode_from_der(mut der: &[u8]) -> Result<Self, DecodeError> {
        if der.split_off_first() != Some(&DER_SEQ) {
            return Err(DecodeError::Der);
        }

        let _ = decode_der_length(&mut der).ok_or(DecodeError::Der)?;
        let n_bytes = decode_der_integer(&mut der).ok_or(DecodeError::Der)?;
        let e_bytes = decode_der_integer(&mut der).ok_or(DecodeError::Der)?;

        let len = NonZero::new(n_bytes.len()).ok_or(DecodeError::Der)?;

        let n = natural_from_bytes_be(n_bytes);
        let e = natural_from_bytes_be(e_bytes);

        Ok(RsaKey { n, e, len })
    }

    pub fn decrypt_block_in(&self, block: &[u8], out: &mut [u8]) -> Result<(), DecryptError> {
        let in_block_len = self.in_block_len();

        if block.len() != in_block_len {
            return Err(DecryptError::Size(in_block_len));
        }

        let c = natural_from_bytes_be(block);

        if c >= self.n {
            return Err(DecryptError::Base);
        }

        let m = (&c).mod_pow(&self.e, &self.n);

        natural_to_bytes_be_in(&m, out);

        Ok(())
    }

    pub fn decrypt_blocks_in_place(&self, blocks: &mut [u8]) -> Result<usize, DecryptError> {
        let in_block_len = self.in_block_len();
        let out_block_len = self.out_block_len();

        if !blocks.len().is_multiple_of(in_block_len) {
            return Err(DecryptError::Size(in_block_len));
        }

        let mut in_block_tail = in_block_len;
        let mut out_block_tail = out_block_len;

        let mut c = Default::default();

        while in_block_tail <= blocks.len() {
            natural_from_bytes_be_in(&blocks[in_block_tail - in_block_len..in_block_tail], &mut c);

            if c >= self.n {
                return Err(DecryptError::Base);
            }

            let m = (&c).mod_pow(&self.e, &self.n);

            natural_to_bytes_be_in(
                &m,
                &mut blocks[out_block_tail - out_block_len..out_block_tail],
            );

            in_block_tail += in_block_len;
            out_block_tail += out_block_len;
        }

        Ok(out_block_tail - out_block_len)
    }
}

impl<'a, R: io::Read> RsaDecryptor<'a, R> {
    pub fn new(key: &'a RsaKey, reader: R) -> Self {
        let in_block_len = key.in_block_len();
        let out_block_len = key.out_block_len();

        let mut out = Cursor::new(vec![0; out_block_len].into_boxed_slice());
        out.set_position(out_block_len as u64);

        let reader = BufReader::with_capacity(in_block_len, reader);

        Self { key, out, reader }
    }
}

impl<R: io::Read> io::Read for RsaDecryptor<'_, R> {
    fn read(&mut self, mut buf: &mut [u8]) -> io::Result<usize> {
        let mut read = 0;

        while !buf.is_empty() {
            let out = self.out.fill_buf()?;

            if !out.is_empty() {
                let amt = buf.write(out)?;
                self.out.consume(amt);
                read += amt;
                continue;
            }

            let block = self.reader.fill_buf()?;

            let in_block_len = self.key.in_block_len();
            let out_block_len = self.key.out_block_len();

            let out = match buf.split_off_mut(..out_block_len) {
                Some(out) => {
                    read += out_block_len;
                    out
                }
                None => {
                    self.out.set_position(0);
                    self.out.get_mut()
                }
            };

            self.key.decrypt_block_in(block, out)?;
            self.reader.consume(in_block_len);
        }

        Ok(read)
    }

    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
        let read = self.out.read_to_end(buf)?;
        let start_at = buf.len();

        self.reader.read_to_end(buf)?;

        let in_place_len = self.key.decrypt_blocks_in_place(&mut buf[start_at..])?;
        buf.truncate(start_at + in_place_len);

        Ok(read + in_place_len)
    }
}

impl From<DecryptError> for io::Error {
    fn from(e: DecryptError) -> Self {
        Self::new(io::ErrorKind::InvalidInput, e)
    }
}

fn decode_der_length(der: &mut &[u8]) -> Option<usize> {
    let first = *der.split_off_first()?;

    if first < 0x80 {
        return Some(first as usize);
    }

    let mut length = 0usize;
    for byte in der.split_off(..(first & 0x7F) as usize)? {
        length = (length << 8) | (*byte as usize);
    }

    Some(length)
}

fn decode_der_integer<'a>(der: &mut &'a [u8]) -> Option<&'a [u8]> {
    if *der.split_off_first()? != DER_INT {
        return None;
    }

    let mut length = decode_der_length(der)?;
    if der.first() == Some(&0) {
        let _ = der.split_off_first();
        length -= 1;
    }

    der.split_off(..length)
}

const LIMB_SIZE: usize = size_of::<Limb>();

fn natural_from_bytes_be(bytes: &[u8]) -> Natural {
    let mut n = Default::default();
    natural_from_bytes_be_in(bytes, &mut n);
    n
}

fn natural_from_bytes_be_in(bytes: &[u8], out: &mut Natural) {
    let mut owned = mem::take(out).into_limbs_asc();
    owned.clear();

    let rchunks = bytes.rchunks_exact(LIMB_SIZE);
    let remainder = rchunks.remainder();

    owned.extend(
        rchunks
            .map(|chunk| Limb::from_be_bytes(*chunk.as_array().unwrap()))
            .chain([{
                let mut limb = 0 as Limb;
                for &b in remainder {
                    limb = (limb << 8) | (b as Limb);
                }
                limb
            }]),
    );

    *out = Natural::from_owned_limbs_asc(owned);
}

fn natural_to_bytes_be_in(n: &Natural, output: &mut [u8]) {
    for (chunk, limb) in output
        .rchunks_mut(LIMB_SIZE)
        .zip(n.limbs().chain(iter::repeat(0)))
    {
        let bytes = limb.to_be_bytes();
        match chunk.first_chunk_mut() {
            Some(chunk) => *chunk = bytes,
            None => chunk.copy_from_slice(&bytes[LIMB_SIZE - chunk.len()..]),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{self, File},
        io::{Read, Seek, SeekFrom},
        str::FromStr,
    };

    use malachite::Natural;
    use xxhash_rust::xxh3::xxh3_128;

    use crate::{
        crypto::rsa::{RsaDecryptor, RsaKey},
        tests::with_steam_game_dir,
    };

    #[test]
    fn rsa_key_from_pem() {
        let pem = fs::read_to_string("dist/dvdbnd/Key/EldenRing_PC/Data0.pem").unwrap();
        let key = RsaKey::decode_from_pem(&pem).unwrap();

        let n = Natural::from_str(
            "30940679651856464003863519012861425075643104058514255042409407775461539164232001464111747802287503994584115460695219946415463757855769544564543133305362667234994320091346600072787846101640908804609674148912640698182204413717023320482555104738657612947097864516619838775105479061066953860376513936345036417796510596250035333309276093916063318025534623353679808835088639253097445600240407055917692164250454748713139380524459276549670495595888014292510224725224961648816514867361168366775220833249571647161213127829995294462573295299531814432054395958249943370671287835978796616838355080595083346711018916036330510565979",
        ).unwrap();
        let e = Natural::from_str("1194458145").unwrap();

        assert_eq!(key.n, n);
        assert_eq!(key.e, e);

        assert_eq!(key.in_block_len(), 256);
        assert_eq!(key.out_block_len(), 255);
    }

    #[test]
    fn steam_game_rsa_decrypt_all() {
        with_ds3_data3(|reader, data3_len| {
            let mut bytes = vec![];
            let bytes_read = reader.read_to_end(&mut bytes).unwrap();

            assert_eq!(bytes_read, bytes.len());
            assert_eq!(bytes_read as u64, data3_len / 256 * 255);

            let hash = xxh3_128(&bytes);

            assert_eq!(hash, 0x9db871d3ca1f6ddc2a4d084e55f9dd28);
        });
    }

    #[test]
    fn steam_game_rsa_decrypt_first_block() {
        with_ds3_data3(|reader, _| {
            let mut bytes = [0; 32];
            reader.read_exact(&mut bytes).unwrap();

            assert_eq!(
                bytes,
                [
                    0x42, 0x48, 0x44, 0x35, 0xFF, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0xA5,
                    0x9F, 0x01, 0x00, 0x67, 0x00, 0x00, 0x00, 0x25, 0x00, 0x00, 0x00, 0x09, 0x00,
                    0x00, 0x00, 0x46, 0x44, 0x50, 0x5F
                ]
            );
        });
    }

    #[test]
    fn steam_game_rsa_decrypt_all_complex() {
        with_ds3_data3(|reader, data3_len| {
            let mut bytes = vec![];
            let mut bytes_read = 0;

            for amt in [
                255, 256, 1, 0, 12, 4, 120, 4000, 3333, 1, 256, 5, 5, 5, 5, 4, 256,
            ] {
                let len = bytes.len();
                bytes.resize(len + amt, 0);
                reader.read_exact(bytes.split_at_mut(len).1).unwrap();
                bytes_read += amt;
            }

            bytes_read += reader.read_to_end(&mut bytes).unwrap();

            assert_eq!(bytes_read, bytes.len());
            assert_eq!(bytes_read as u64, data3_len / 256 * 255);

            let hash = xxh3_128(&bytes);

            assert_eq!(hash, 0x9db871d3ca1f6ddc2a4d084e55f9dd28);
        });
    }

    #[track_caller]
    fn with_ds3_data3<F>(f: F)
    where
        F: FnOnce(&mut RsaDecryptor<'_, File>, u64),
    {
        with_steam_game_dir(374320, |install_dir| {
            let pem = fs::read_to_string("dist/dvdbnd/Key/DarkSouls3_PC/Data3.pem").unwrap();
            let key = RsaKey::decode_from_pem(&pem).unwrap();

            let (data3, data3_len) = {
                let mut data3 = File::open(install_dir.join("Game/Data3.bhd")).unwrap();

                let old_pos = data3.stream_position().unwrap();
                let len = data3.seek(SeekFrom::End(0)).unwrap();
                data3.seek(SeekFrom::Start(old_pos)).unwrap();

                (data3, len)
            };

            let mut reader = RsaDecryptor::new(&key, data3);

            f(&mut reader, data3_len)
        });
    }
}
