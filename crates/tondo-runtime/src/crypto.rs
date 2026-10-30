//! What `std.crypto` asks of the runtime (standard library 14.34): ring's
//! sealing, key derivation and signatures, and the hashing of passwords
//! with Argon2id and the checking of bcrypt's. Keys come and go as bytes:
//! PKCS #8 for private keys and each kind's raw form for public ones.

use super::{ArrayHeader, Str, element, str_bytes, str_new};
use crate::io::byte_array;
use ring::{aead, hkdf, pbkdf2, rand, signature};
use std::num::NonZeroU32;

fn bytes(array: *mut ArrayHeader) -> Vec<u8> {
    if array.is_null() {
        return Vec::new();
    }
    unsafe { (0..(*array).len).map(|i| *element(array, 1, i)).collect() }
}

fn text(s: *const Str) -> String {
    String::from_utf8_lossy(unsafe { str_bytes(s) }).into_owned()
}

fn cipher(code: i64) -> &'static aead::Algorithm {
    match code {
        1 => &aead::CHACHA20_POLY1305,
        _ => &aead::AES_256_GCM,
    }
}

/// The data encrypted with a key of 32 bytes and a nonce of 12, then the
/// tag of 16.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_crypto_seal(
    code: i64,
    key: *mut ArrayHeader,
    nonce: *mut ArrayHeader,
    data: *mut ArrayHeader,
    context: *mut ArrayHeader,
) -> *mut ArrayHeader {
    let key = aead::LessSafeKey::new(aead::UnboundKey::new(cipher(code), &bytes(key)).expect("a key of 32 bytes"));
    let nonce = aead::Nonce::try_assume_unique_for_key(&bytes(nonce)).expect("a nonce of 12 bytes");
    let mut sealed = bytes(data);
    key.seal_in_place_append_tag(nonce, aead::Aad::from(bytes(context)), &mut sealed).expect("data it can seal");
    byte_array(&sealed)
}

/// 1 and the data that `seal` sealed, or 0 when it does not open.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_crypto_open(
    code: i64,
    key: *mut ArrayHeader,
    nonce: *mut ArrayHeader,
    sealed: *mut ArrayHeader,
    context: *mut ArrayHeader,
) -> *mut ArrayHeader {
    let key = aead::LessSafeKey::new(aead::UnboundKey::new(cipher(code), &bytes(key)).expect("a key of 32 bytes"));
    let Ok(nonce) = aead::Nonce::try_assume_unique_for_key(&bytes(nonce)) else { return byte_array(&[0]) };
    let mut data = bytes(sealed);
    match key.open_in_place(nonce, aead::Aad::from(bytes(context)), &mut data) {
        Ok(opened) => byte_array(&[&[1], &opened[..]].concat()),
        Err(_) => byte_array(&[0]),
    }
}

/// HKDF with SHA-256: `length` bytes, at most 255 times 32.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_crypto_hkdf(
    secret: *mut ArrayHeader,
    salt: *mut ArrayHeader,
    info: *mut ArrayHeader,
    length: i64,
) -> *mut ArrayHeader {
    struct Length(usize);
    impl hkdf::KeyType for Length {
        fn len(&self) -> usize {
            self.0
        }
    }
    let length = (length as usize).min(255 * 32);
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, &bytes(salt)).extract(&bytes(secret));
    let info = bytes(info);
    let mut out = vec![0; length];
    prk.expand(&[&info], Length(length)).and_then(|okm| okm.fill(&mut out)).expect("a length HKDF gives");
    byte_array(&out)
}

/// PBKDF2 with HMAC-SHA-256.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_crypto_pbkdf2(
    password: *mut ArrayHeader,
    salt: *mut ArrayHeader,
    iterations: i64,
    length: i64,
) -> *mut ArrayHeader {
    let iterations = NonZeroU32::new(iterations.clamp(1, u32::MAX as i64) as u32).expect("at least one");
    let mut out = vec![0; length.max(0) as usize];
    pbkdf2::derive(pbkdf2::PBKDF2_HMAC_SHA256, iterations, &bytes(salt), &bytes(password), &mut out);
    byte_array(&out)
}

/// A new key's PKCS #8: Ed25519 (0), ECDSA on P-256 (1) or P-384 (2).
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_crypto_generate(kind: i64) -> *mut ArrayHeader {
    let random = rand::SystemRandom::new();
    let document = match kind {
        0 => signature::Ed25519KeyPair::generate_pkcs8(&random),
        1 => signature::EcdsaKeyPair::generate_pkcs8(&signature::ECDSA_P256_SHA256_FIXED_SIGNING, &random),
        _ => signature::EcdsaKeyPair::generate_pkcs8(&signature::ECDSA_P384_SHA384_FIXED_SIGNING, &random),
    };
    byte_array(document.expect("the system's entropy").as_ref())
}

enum Signer {
    Ed25519(signature::Ed25519KeyPair),
    Ecdsa(signature::EcdsaKeyPair),
    Rsa(signature::RsaKeyPair),
}

fn signer(kind: i64, pkcs8: &[u8]) -> Option<Signer> {
    let random = rand::SystemRandom::new();
    match kind {
        0 => signature::Ed25519KeyPair::from_pkcs8_maybe_unchecked(pkcs8).ok().map(Signer::Ed25519),
        1 => signature::EcdsaKeyPair::from_pkcs8(&signature::ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8, &random)
            .ok()
            .map(Signer::Ecdsa),
        2 => signature::EcdsaKeyPair::from_pkcs8(&signature::ECDSA_P384_SHA384_FIXED_SIGNING, pkcs8, &random)
            .ok()
            .map(Signer::Ecdsa),
        _ => signature::RsaKeyPair::from_pkcs8(pkcs8).ok().map(Signer::Rsa),
    }
}

/// The public key of a private one, in its raw form: Ed25519's 32 bytes,
/// an uncompressed point, or RSA's PKCS #1; empty when the key is not
/// valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_crypto_public(kind: i64, pkcs8: *mut ArrayHeader) -> *mut ArrayHeader {
    use signature::KeyPair;
    match signer(kind, &bytes(pkcs8)) {
        Some(Signer::Ed25519(pair)) => byte_array(pair.public_key().as_ref()),
        Some(Signer::Ecdsa(pair)) => byte_array(pair.public_key().as_ref()),
        Some(Signer::Rsa(pair)) => byte_array(pair.public_key().as_ref()),
        None => byte_array(&[]),
    }
}

/// The signature of the data.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_crypto_sign(
    kind: i64,
    pkcs8: *mut ArrayHeader,
    data: *mut ArrayHeader,
) -> *mut ArrayHeader {
    let random = rand::SystemRandom::new();
    let data = bytes(data);
    match signer(kind, &bytes(pkcs8)) {
        Some(Signer::Ed25519(pair)) => byte_array(pair.sign(&data).as_ref()),
        Some(Signer::Ecdsa(pair)) => byte_array(pair.sign(&random, &data).expect("the system's entropy").as_ref()),
        Some(Signer::Rsa(pair)) => {
            let mut out = vec![0; pair.public().modulus_len()];
            pair.sign(&signature::RSA_PKCS1_SHA256, &random, &data, &mut out).expect("a key that signs");
            byte_array(&out)
        }
        None => byte_array(&[]),
    }
}

/// Whether the signature is the data's for the raw public key.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_crypto_verify(
    kind: i64,
    public: *mut ArrayHeader,
    data: *mut ArrayHeader,
    signed: *mut ArrayHeader,
) -> u8 {
    let algorithm: &dyn signature::VerificationAlgorithm = match kind {
        0 => &signature::ED25519,
        1 => &signature::ECDSA_P256_SHA256_FIXED,
        2 => &signature::ECDSA_P384_SHA384_FIXED,
        _ => &signature::RSA_PKCS1_2048_8192_SHA256,
    };
    let key = signature::UnparsedPublicKey::new(algorithm, bytes(public));
    key.verify(&bytes(data), &bytes(signed)).is_ok() as u8
}

/// The password hashed with Argon2id, OWASP's parameters and a random salt
/// of 16 bytes, in PHC's format.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_crypto_hash_password(password: *const Str) -> *mut Str {
    use argon2::password_hash::PasswordHasher;
    use ring::rand::SecureRandom;
    let mut salt = [0u8; 16];
    rand::SystemRandom::new().fill(&mut salt).expect("the system's entropy");
    let hashed = argon2::Argon2::default().hash_password_with_salt(text(password).as_bytes(), &salt);
    str_new(hashed.expect("a password Argon2 hashes").to_string().as_bytes())
}

/// Whether the password is the one of an Argon2 or bcrypt hash.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_crypto_verify_password(password: *const Str, hash: *const Str) -> u8 {
    use argon2::password_hash::PasswordVerifier;
    let (password, hash) = (text(password), text(hash));
    if hash.starts_with("$2") {
        return bcrypt::verify(&password, &hash).unwrap_or(false) as u8;
    }
    argon2::Argon2::default().verify_password(password.as_bytes(), hash.as_str()).is_ok() as u8
}
