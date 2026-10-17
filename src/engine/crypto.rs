//! Standard block ciphers for the script `DECRYPT` / `ENCRYPT` commands.
//!
//! Ported from the reference patch interpreter's decrypt step: the AES,
//! Blowfish and 3DES wrappers it exposes. Only the ciphers the bundled codes
//! actually use are implemented: AES and Blowfish in ECB and CBC, and 3DES in
//! ECB and CBC. They run with no padding, over as many whole blocks as fit the
//! range, which is what the reference does.
//!
//! Every other cipher (Camellia, AES-CTR, and the game-specific schemes such as
//! `ffxiii`, `mgs` or `dw8xl`) is refused with a clear message rather than
//! guessed at, so a cheat never silently writes scrambled bytes.

use super::ApplyError;
use aes::{Aes128, Aes192, Aes256};
use blowfish::Blowfish;
use cipher::generic_array::GenericArray;
use cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use des::TdesEde3;

/// Which way a cipher runs. `DECRYPT` and `ENCRYPT` share one code path.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    Decrypt,
    Encrypt,
}

impl Dir {
    fn verb(self) -> &'static str {
        match self {
            Dir::Decrypt => "decrypt",
            Dir::Encrypt => "encrypt",
        }
    }
}

/// The block cipher named in a `decrypt`/`encrypt` command.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Block {
    Aes,
    Blowfish,
    Des3,
}

/// The chaining mode. `blowfish_ecb` is short for `(Block::Blowfish, Ecb)`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Chaining {
    Ecb,
    Cbc,
}

/// Run `block`/`chaining` over `data` in place.
///
/// `iv` is only used by CBC; an ECB caller passes an empty slice. A key or IV of
/// the wrong size is an error, matching the way the reference silently does
/// nothing (we would rather tell the user than write a half-processed file).
pub fn run(
    dir: Dir,
    block: Block,
    chaining: Chaining,
    key: &[u8],
    iv: &[u8],
    data: &mut [u8],
) -> Result<(), ApplyError> {
    match block {
        Block::Aes => match key.len() {
            16 => run_aes::<Aes128>(dir, chaining, key, iv, data),
            24 => run_aes::<Aes192>(dir, chaining, key, iv, data),
            32 => run_aes::<Aes256>(dir, chaining, key, iv, data),
            n => Err(ApplyError::new(format!(
                "AES key must be 16, 24 or 32 bytes (got {n})"
            ))),
        },
        Block::Blowfish => match chaining {
            Chaining::Ecb => ecb::<Blowfish>(dir, key, data),
            Chaining::Cbc => cbc::<Blowfish>(dir, key, iv, data),
        },
        Block::Des3 => {
            if key.len() != 24 {
                return Err(ApplyError::new(format!(
                    "3DES needs a 24-byte key (got {})",
                    key.len()
                )));
            }
            match chaining {
                Chaining::Ecb => ecb::<TdesEde3>(dir, key, data),
                Chaining::Cbc => cbc::<TdesEde3>(dir, key, iv, data),
            }
        }
    }
}

/// A cipher this tool does not implement, as named in the patch text.
pub fn unsupported(name: &str) -> ApplyError {
    let lower = name.to_ascii_lowercase();
    ApplyError::new(format!(
        "this save uses the '{lower}' encryption, which this tool cannot decrypt or re-encrypt yet"
    ))
}

fn run_aes<C>(
    dir: Dir,
    chaining: Chaining,
    key: &[u8],
    iv: &[u8],
    data: &mut [u8],
) -> Result<(), ApplyError>
where
    C: BlockEncrypt + BlockDecrypt + KeyInit,
{
    match chaining {
        Chaining::Ecb => ecb::<C>(dir, key, data),
        Chaining::Cbc => cbc::<C>(dir, key, iv, data),
    }
}

fn ecb<C>(dir: Dir, key: &[u8], data: &mut [u8]) -> Result<(), ApplyError>
where
    C: BlockEncrypt + BlockDecrypt + KeyInit,
{
    let cipher =
        C::new_from_slice(key).map_err(|_| ApplyError::new("invalid encryption key"))?;
    let bs = C::block_size();
    for chunk in data.chunks_exact_mut(bs) {
        let block = GenericArray::from_mut_slice(chunk);
        match dir {
            Dir::Decrypt => cipher.decrypt_block(block),
            Dir::Encrypt => cipher.encrypt_block(block),
        }
    }
    Ok(())
}

fn cbc<C>(dir: Dir, key: &[u8], iv: &[u8], data: &mut [u8]) -> Result<(), ApplyError>
where
    C: BlockEncrypt + BlockDecrypt + KeyInit,
{
    let bs = C::block_size();
    if iv.len() != bs {
        return Err(ApplyError::new(format!(
            "{} CBC needs a {}-byte IV (got {})",
            dir.verb(),
            bs,
            iv.len()
        )));
    }
    let cipher =
        C::new_from_slice(key).map_err(|_| ApplyError::new("invalid encryption key"))?;
    let mut prev = iv.to_vec();

    for chunk in data.chunks_exact_mut(bs) {
        match dir {
            Dir::Decrypt => {
                let mut block = GenericArray::clone_from_slice(chunk);
                cipher.decrypt_block(&mut block);
                let ciphertext = chunk.to_vec();
                for i in 0..bs {
                    chunk[i] = block[i] ^ prev[i];
                }
                prev = ciphertext;
            }
            Dir::Encrypt => {
                for i in 0..bs {
                    chunk[i] ^= prev[i];
                }
                let mut block = GenericArray::clone_from_slice(chunk);
                cipher.encrypt_block(&mut block);
                chunk.copy_from_slice(&block);
                prev.copy_from_slice(chunk);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key16() -> Vec<u8> {
        (0u8..16).collect()
    }

    #[test]
    fn aes_ecb_round_trips() {
        let key = key16();
        let original: Vec<u8> = (0..64).map(|i| (i * 7) as u8).collect();
        let mut data = original.clone();
        run(Dir::Encrypt, Block::Aes, Chaining::Ecb, &key, &[], &mut data).unwrap();
        assert_ne!(data, original);
        run(Dir::Decrypt, Block::Aes, Chaining::Ecb, &key, &[], &mut data).unwrap();
        assert_eq!(data, original);
    }

    #[test]
    fn aes_cbc_round_trips() {
        let key = key16();
        let iv: Vec<u8> = (16u8..32).collect();
        let original: Vec<u8> = (0..64).map(|i| (i * 5) as u8).collect();
        let mut data = original.clone();
        run(Dir::Encrypt, Block::Aes, Chaining::Cbc, &key, &iv, &mut data).unwrap();
        run(Dir::Decrypt, Block::Aes, Chaining::Cbc, &key, &iv, &mut data).unwrap();
        assert_eq!(data, original);
    }

    #[test]
    fn blowfish_ecb_round_trips() {
        // Blowfish keys are ASCII strings; variable length is allowed.
        let key = b"SBmdYgEamc=#sA0)Mhs9#>/4iiXbMPxW";
        let original: Vec<u8> = (0..64).map(|i| (i * 11) as u8).collect();
        let mut data = original.clone();
        run(Dir::Encrypt, Block::Blowfish, Chaining::Ecb, key, &[], &mut data).unwrap();
        run(Dir::Decrypt, Block::Blowfish, Chaining::Ecb, key, &[], &mut data).unwrap();
        assert_eq!(data, original);
    }

    #[test]
    fn des3_cbc_round_trips() {
        let key: Vec<u8> = (0..24).collect();
        let iv: Vec<u8> = (0..8).collect();
        let original: Vec<u8> = (0..64).map(|i| (i * 3) as u8).collect();
        let mut data = original.clone();
        run(Dir::Encrypt, Block::Des3, Chaining::Cbc, &key, &iv, &mut data).unwrap();
        run(Dir::Decrypt, Block::Des3, Chaining::Cbc, &key, &iv, &mut data).unwrap();
        assert_eq!(data, original);
    }

    #[test]
    fn non_block_length_only_covers_whole_blocks() {
        let key = key16();
        let mut data = vec![0xAAu8; 20];
        run(Dir::Encrypt, Block::Aes, Chaining::Ecb, &key, &[], &mut data).unwrap();
        // The last 4 bytes are not part of a block and stay untouched.
        assert_eq!(&data[16..], &[0xAA, 0xAA, 0xAA, 0xAA]);
    }

    #[test]
    fn bad_key_is_refused() {
        let mut data = vec![0u8; 16];
        let err = run(Dir::Decrypt, Block::Aes, Chaining::Ecb, &[0u8; 5], &[], &mut data).unwrap_err();
        assert!(err.0.contains("AES key"));
    }
}
