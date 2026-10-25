// WoW SRP6 compatibility follows wow_srp 0.7.0; see ../LICENSE-wow_srp.
use anyhow::{ensure, Result};
use num_bigint::BigUint;
use sha1::{Digest, Sha1};
use subtle::ConstantTimeEq;

pub const GENERATOR: u8 = 7;
pub const PRIME: [u8; 32] = [
    0xb7, 0x9b, 0x3e, 0x2a, 0x87, 0x82, 0x3c, 0xab, 0x8f, 0x5e, 0xbf, 0xbf, 0x8e, 0xb1, 0x01, 0x08,
    0x53, 0x50, 0x06, 0x29, 0x8b, 0x5b, 0xad, 0xbd, 0x5b, 0x53, 0xe1, 0x89, 0x5e, 0x64, 0x4b, 0x89,
];

pub fn normalize(username: &str) -> Result<String> {
    ensure!(
        (1..=16).contains(&username.len())
            && username
                .bytes()
                .all(|byte| byte.is_ascii() && !byte.is_ascii_control()),
        "invalid SRP Account name"
    );
    Ok(username.to_ascii_uppercase())
}

fn padded(value: BigUint) -> [u8; 32] {
    let mut bytes = [0; 32];
    let value = value.to_bytes_le();
    bytes[..value.len()].copy_from_slice(&value);
    bytes
}

/// `private_key` must come from the host's cryptographic random source for each challenge.
pub fn server_public_key(verifier: &[u8; 32], private_key: &[u8; 32]) -> Result<[u8; 32]> {
    let prime = BigUint::from_bytes_le(&PRIME);
    let verifier = BigUint::from_bytes_le(verifier);
    let private = BigUint::from_bytes_le(private_key);
    ensure!(
        verifier != BigUint::ZERO && verifier < prime && private != BigUint::ZERO,
        "invalid SRP verifier or private key"
    );
    let public = (BigUint::from(3u8) * verifier
        + BigUint::from(GENERATOR).modpow(&private, &prime))
        % &prime;
    ensure!(public != BigUint::ZERO, "invalid SRP public key");
    Ok(padded(public))
}

pub struct SessionProof {
    pub session_key: [u8; 40],
    pub server_proof: [u8; 20],
}

pub fn verify(
    username: &str,
    salt: &[u8; 32],
    verifier: &[u8; 32],
    private_key: &[u8; 32],
    client_public_key: &[u8; 32],
    client_proof: &[u8; 20],
) -> Result<SessionProof> {
    let username = normalize(username)?;
    let server_public_key = server_public_key(verifier, private_key)?;
    let prime = BigUint::from_bytes_le(&PRIME);
    let client = BigUint::from_bytes_le(client_public_key);
    ensure!(
        &client % &prime != BigUint::ZERO,
        "invalid SRP client public key"
    );
    let scrambling = Sha1::new()
        .chain_update(client_public_key)
        .chain_update(server_public_key)
        .finalize();
    let scrambling = BigUint::from_bytes_le(&scrambling);
    let verifier = BigUint::from_bytes_le(verifier);
    let private = BigUint::from_bytes_le(private_key);
    let shared = (client * verifier.modpow(&scrambling, &prime)).modpow(&private, &prime);
    ensure!(shared != BigUint::ZERO, "invalid SRP shared key");
    let session_key = interleave(&padded(shared));
    let prime_hash = Sha1::digest(PRIME);
    let generator_hash = Sha1::digest([GENERATOR]);
    let xor: [u8; 20] = std::array::from_fn(|index| prime_hash[index] ^ generator_hash[index]);
    let expected: [u8; 20] = Sha1::new()
        .chain_update(xor)
        .chain_update(Sha1::digest(username.as_bytes()))
        .chain_update(salt)
        .chain_update(client_public_key)
        .chain_update(server_public_key)
        .chain_update(session_key)
        .finalize()
        .into();
    ensure!(
        bool::from(expected.ct_eq(client_proof)),
        "SRP proof does not match"
    );
    let server_proof = Sha1::new()
        .chain_update(client_public_key)
        .chain_update(client_proof)
        .chain_update(session_key)
        .finalize()
        .into();
    Ok(SessionProof {
        session_key,
        server_proof,
    })
}

fn interleave(shared: &[u8; 32]) -> [u8; 40] {
    let start = shared
        .iter()
        .position(|byte| *byte != 0)
        .unwrap_or(shared.len());
    // Vanilla skips an even number of leading zero bytes in the little-endian shared key.
    let shared = &shared[start.next_multiple_of(2)..];
    let even: Vec<u8> = shared.iter().copied().step_by(2).collect();
    let odd: Vec<u8> = shared.iter().copied().skip(1).step_by(2).collect();
    let even = Sha1::digest(even);
    let odd = Sha1::digest(odd);
    std::array::from_fn(|index| {
        if index % 2 == 0 {
            even[index / 2]
        } else {
            odd[index / 2]
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_srp::{
        client::SrpClientChallenge, normalized_string::NormalizedString, server::SrpVerifier,
        PublicKey,
    };

    #[test]
    fn wow_srp_clients_accept_the_module_proof_and_derive_the_same_session_key() {
        let username = NormalizedString::new("NATIVE").unwrap();
        let password = NormalizedString::new("PASSWORD").unwrap();
        for private in [[1; 32], [0x57; 32], [0xff; 32]] {
            let verifier =
                SrpVerifier::from_username_and_password(username.clone(), password.clone());
            let public = server_public_key(verifier.password_verifier(), &private).unwrap();
            let client = SrpClientChallenge::new(
                username.clone(),
                password.clone(),
                GENERATOR,
                PRIME,
                PublicKey::from_le_bytes(public).unwrap(),
                *verifier.salt(),
            );
            let proof = verify(
                "native",
                verifier.salt(),
                verifier.password_verifier(),
                &private,
                client.client_public_key(),
                client.client_proof(),
            )
            .unwrap();
            let client = client.verify_server_proof(proof.server_proof).unwrap();
            assert_eq!(*client.session_key(), proof.session_key);
        }
    }

    #[test]
    fn invalid_keys_and_wrong_proofs_are_refused() {
        assert!(server_public_key(&[0; 32], &[1; 32]).is_err());
        assert!(server_public_key(&PRIME, &[1; 32]).is_err());
        assert!(server_public_key(&[1; 32], &[0; 32]).is_err());
        for public in [[0; 32], PRIME, [1; 32]] {
            assert!(verify("NATIVE", &[2; 32], &[3; 32], &[4; 32], &public, &[0; 20]).is_err());
        }
        assert!(normalize("").is_err());
        assert!(normalize("NATIVE\0").is_err());
        assert!(normalize("å").is_err());
    }

    #[test]
    fn interleaving_skips_zero_bytes_in_even_pairs() {
        let mut shared = [0; 32];
        shared[1] = 5;
        shared[2..].fill(7);
        let digest = Sha1::digest([7; 15]);
        let expected: [u8; 40] = std::array::from_fn(|index| digest[index / 2]);
        assert_eq!(interleave(&shared), expected);
        shared[1] = 0;
        assert_eq!(interleave(&shared), expected);
    }
}
