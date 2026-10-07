//! The identity export: a Device's secret key, and nothing else, sealed under a password so
//! it can be carried to another install (spec section 2).
//!
//! ```text
//! "BHID" | version u8 | m_cost u32 | t_cost u32 | p_cost u32 | salt [16] | nonce [12] | sealed key [32 + 16]
//! ```
//!
//! Numbers are big-endian; the Argon2id parameters are in the file so they can be raised
//! later. The password goes through Argon2id to a 32-byte key, which seals the secret key with
//! AES-256-GCM. Everything before the sealed key is authenticated as associated data, so a
//! changed byte anywhere makes the file fail to open. The core refuses only an empty password;
//! the UI asks for at least 8 characters.

use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use argon2::{Algorithm, Argon2, Params, Version};
use zeroize::Zeroizing;

const MAGIC: &[u8; 4] = b"BHID";
const VERSION: u8 = 1;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;
const TAG_LEN: usize = 16;
const HEADER_LEN: usize = MAGIC.len() + 1 + 3 * 4 + SALT_LEN + NONCE_LEN;
const FILE_LEN: usize = HEADER_LEN + KEY_LEN + TAG_LEN;

/// Argon2id cost: what a new export is made with, and (as the file is read) what an import
/// refuses to exceed, so a damaged or hostile file cannot make it allocate gigabytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cost {
    /// Memory, in KiB.
    m: u32,
    t: u32,
    p: u32,
}

const EXPORT_COST: Cost = Cost { m: 64 * 1024, t: 3, p: 1 };
const MAX_COST: Cost = Cost { m: 512 * 1024, t: 10, p: 8 };

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum IdentityFileError {
    #[error("An identity export needs a password.")]
    EmptyPassword,
    #[error("That is not a BhayanakShare identity file.")]
    NotAnIdentityFile,
    #[error("That identity file was made by a newer BhayanakShare. Update this one to open it.")]
    UnsupportedVersion,
    /// The password does not open the file, or the file was changed: AES-GCM cannot tell the
    /// two apart.
    #[error("Wrong password, or the file is damaged.")]
    WrongPassword,
}

/// Seals `secret` under `password`, with a fresh salt and nonce.
pub(crate) fn seal(secret: &[u8; KEY_LEN], password: &str) -> Result<Vec<u8>, IdentityFileError> {
    seal_with(secret, password, EXPORT_COST)
}

fn seal_with(secret: &[u8; KEY_LEN], password: &str, cost: Cost) -> Result<Vec<u8>, IdentityFileError> {
    if password.is_empty() {
        return Err(IdentityFileError::EmptyPassword);
    }
    let salt: [u8; SALT_LEN] = rand::random();
    let nonce: [u8; NONCE_LEN] = rand::random();
    let mut file = Vec::with_capacity(FILE_LEN);
    file.extend_from_slice(MAGIC);
    file.push(VERSION);
    for n in [cost.m, cost.t, cost.p] {
        file.extend_from_slice(&n.to_be_bytes());
    }
    file.extend_from_slice(&salt);
    file.extend_from_slice(&nonce);
    debug_assert_eq!(file.len(), HEADER_LEN);

    let key = derive(password, &salt, cost).ok_or(IdentityFileError::NotAnIdentityFile)?;
    let sealed = Aes256Gcm::new(key.as_slice().into())
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: secret, aad: &file })
        .expect("sealing a buffer in memory cannot fail");
    file.extend_from_slice(&sealed);
    Ok(file)
}

/// Opens an identity file: the secret key it holds.
pub(crate) fn open(file: &[u8], password: &str) -> Result<Zeroizing<[u8; KEY_LEN]>, IdentityFileError> {
    if password.is_empty() {
        return Err(IdentityFileError::EmptyPassword);
    }
    if file.len() != FILE_LEN || &file[..MAGIC.len()] != MAGIC {
        return Err(IdentityFileError::NotAnIdentityFile);
    }
    if file[MAGIC.len()] != VERSION {
        return Err(IdentityFileError::UnsupportedVersion);
    }
    let number = |at: usize| u32::from_be_bytes(file[at..at + 4].try_into().expect("four bytes"));
    let params_at = MAGIC.len() + 1;
    let cost = Cost { m: number(params_at), t: number(params_at + 4), p: number(params_at + 8) };
    if cost.m > MAX_COST.m || cost.t > MAX_COST.t || cost.p > MAX_COST.p {
        return Err(IdentityFileError::NotAnIdentityFile);
    }
    let (header, sealed) = file.split_at(HEADER_LEN);
    let salt = &header[params_at + 12..params_at + 12 + SALT_LEN];
    let nonce = &header[HEADER_LEN - NONCE_LEN..];

    let key = derive(password, salt, cost).ok_or(IdentityFileError::NotAnIdentityFile)?;
    let plain = Aes256Gcm::new(key.as_slice().into())
        .decrypt(Nonce::from_slice(nonce), Payload { msg: sealed, aad: header })
        .map(Zeroizing::new)
        .map_err(|_| IdentityFileError::WrongPassword)?;
    let secret: [u8; KEY_LEN] = plain.as_slice().try_into().map_err(|_| IdentityFileError::NotAnIdentityFile)?;
    Ok(Zeroizing::new(secret))
}

/// `None` when the parameters are not ones Argon2 can run with (a zero cost, say).
fn derive(password: &str, salt: &[u8], cost: Cost) -> Option<Zeroizing<[u8; KEY_LEN]>> {
    let params = Params::new(cost.m, cost.t, cost.p, Some(KEY_LEN)).ok()?;
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, key.as_mut_slice())
        .ok()?;
    Some(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cheap enough for many derivations in an unoptimised test build.
    const TEST_COST: Cost = Cost { m: 32, t: 1, p: 1 };
    const SECRET: [u8; 32] = [42; 32];
    const PASSWORD: &str = "correct horse battery";

    fn sealed() -> Vec<u8> {
        seal_with(&SECRET, PASSWORD, TEST_COST).unwrap()
    }

    #[test]
    fn round_trips() {
        let file = sealed();
        assert_eq!(file.len(), FILE_LEN);
        assert_eq!(*open(&file, PASSWORD).unwrap(), SECRET);
    }

    #[test]
    fn every_file_gets_its_own_salt_and_nonce() {
        assert_ne!(sealed(), sealed());
    }

    #[test]
    fn the_file_holds_the_parameters_and_not_the_key() {
        let file = seal(&SECRET, PASSWORD).unwrap();
        assert_eq!(&file[..4], b"BHID");
        assert_eq!(file[4], 1);
        assert_eq!(u32::from_be_bytes(file[5..9].try_into().unwrap()), 64 * 1024);
        assert_eq!(u32::from_be_bytes(file[9..13].try_into().unwrap()), 3);
        assert_eq!(u32::from_be_bytes(file[13..17].try_into().unwrap()), 1);
        assert!(!file.windows(32).any(|w| w == SECRET));
        // What an export is made with is what an import reads back, at the real cost.
        assert_eq!(*open(&file, PASSWORD).unwrap(), SECRET);
    }

    #[test]
    fn a_wrong_password_is_an_error() {
        assert_eq!(open(&sealed(), "not the password").unwrap_err(), IdentityFileError::WrongPassword);
    }

    #[test]
    fn an_empty_password_is_refused_both_ways() {
        assert_eq!(seal_with(&SECRET, "", TEST_COST).unwrap_err(), IdentityFileError::EmptyPassword);
        assert_eq!(open(&sealed(), "").unwrap_err(), IdentityFileError::EmptyPassword);
    }

    #[test]
    fn a_changed_byte_anywhere_fails_to_open() {
        let file = sealed();
        for at in 0..file.len() {
            let mut damaged = file.clone();
            damaged[at] ^= 1;
            assert!(open(&damaged, PASSWORD).is_err(), "byte {at} changed, and it still opened");
        }
    }

    #[test]
    fn what_a_changed_byte_means_depends_on_where() {
        let file = sealed();
        let flip = |at: usize| {
            let mut damaged = file.clone();
            damaged[at] ^= 1;
            open(&damaged, PASSWORD).unwrap_err()
        };
        assert_eq!(flip(0), IdentityFileError::NotAnIdentityFile); // magic
        assert_eq!(flip(4), IdentityFileError::UnsupportedVersion);
        assert_eq!(flip(HEADER_LEN - NONCE_LEN - 1), IdentityFileError::WrongPassword); // salt
        assert_eq!(flip(HEADER_LEN - 1), IdentityFileError::WrongPassword); // nonce
        assert_eq!(flip(HEADER_LEN), IdentityFileError::WrongPassword); // sealed key
        assert_eq!(flip(FILE_LEN - 1), IdentityFileError::WrongPassword); // tag
    }

    #[test]
    fn a_changed_cost_parameter_fails_to_open() {
        let file = sealed();
        // Just under the limit, so the file is read as far as the check on its contents.
        let mut damaged = file.clone();
        damaged[9..13].copy_from_slice(&2u32.to_be_bytes()); // t = 1 -> 2
        assert_eq!(open(&damaged, PASSWORD).unwrap_err(), IdentityFileError::WrongPassword);
    }

    #[test]
    fn a_cost_above_the_limit_is_refused_before_it_is_run() {
        let mut file = sealed();
        file[5..9].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(open(&file, PASSWORD).unwrap_err(), IdentityFileError::NotAnIdentityFile);
        let mut file = sealed();
        file[13..17].copy_from_slice(&0u32.to_be_bytes()); // no parallelism at all
        assert_eq!(open(&file, PASSWORD).unwrap_err(), IdentityFileError::NotAnIdentityFile);
    }

    #[test]
    fn a_truncated_or_extended_file_is_not_an_identity_file() {
        let file = sealed();
        for len in [0, 3, 4, HEADER_LEN, FILE_LEN - 1] {
            assert_eq!(open(&file[..len], PASSWORD).unwrap_err(), IdentityFileError::NotAnIdentityFile, "{len} bytes");
        }
        let mut longer = file;
        longer.push(0);
        assert_eq!(open(&longer, PASSWORD).unwrap_err(), IdentityFileError::NotAnIdentityFile);
    }

    #[test]
    fn something_else_entirely_is_not_an_identity_file() {
        assert_eq!(open(&[7; FILE_LEN], PASSWORD).unwrap_err(), IdentityFileError::NotAnIdentityFile);
        assert_eq!(open(b"just some text", PASSWORD).unwrap_err(), IdentityFileError::NotAnIdentityFile);
    }
}
