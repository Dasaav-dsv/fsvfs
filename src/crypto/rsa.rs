use malachite::{Natural, platform::Limb};
use thiserror::Error;

#[derive(Clone, Debug)]
pub struct RsaKey {
    n: Natural,
    e: Natural,
    size: usize,
}

#[derive(Debug, Error)]
pub enum DecodeError {
    #[error(transparent)]
    Base64(#[from] base64::DecodeError),

    #[error("malformed DER sequence or encoding")]
    Der,
}

const DER_INT: u8 = 0x02;
const DER_SEQ: u8 = 0x30;

impl RsaKey {
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

        let n = natural_from_bytes_be(n_bytes);
        let e = natural_from_bytes_be(e_bytes);
        let size = n_bytes.len();

        Ok(RsaKey { n, e, size })
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

fn natural_from_bytes_be(bytes: &[u8]) -> Natural {
    const LIMB_SIZE: usize = size_of::<Limb>();

    let rchunks = bytes.rchunks_exact(LIMB_SIZE);
    let remainder = rchunks.remainder();
    let limbs = rchunks
        .map(|chunk| Limb::from_be_bytes(chunk.try_into().unwrap()))
        .chain([{
            let mut limb = 0 as Limb;
            for &b in remainder {
                limb = (limb << 8) | (b as Limb);
            }
            limb
        }])
        .collect();

    Natural::from_owned_limbs_asc(limbs)
}

#[cfg(test)]
mod tests {
    use std::{fs, str::FromStr};

    use malachite::Natural;

    use crate::crypto::rsa::RsaKey;

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
        assert_eq!(key.size, 256);
    }
}
