use ic_types::crypto::{AlgorithmId, UserPublicKey};
use lazy_static::lazy_static;
use rand::SeedableRng;
use rand_chacha::ChaChaRng;

use ic_crypto_internal_types::sign::eddsa::ed25519::{PublicKey, SecretKey};
use ic_crypto_utils_basic_sig::conversions::Ed25519SecretKeyConversions;

const PRIVATE_KEY: &str = "-----BEGIN PRIVATE KEY-----\nMFMCAQEwBQYDK2VwBCIEILhMGpmYuJ0JEhDwocj6pxxOmIpGAXZd40AjkNhuae6q\noSMDIQBeXC6ae2dkJ8QC50bBjlyLqsFQFsMsIThWB21H6t6JRA==\n-----END PRIVATE KEY-----";

// get public key from the dedicated whitelisted private key used by the
// workload generator
pub fn get_pub(private_key: Option<&str>) -> PublicKey {
    let contents = private_key.unwrap_or(PRIVATE_KEY);
    let (_secret_key, public_key) = SecretKey::from_pem(contents).expect("Invalid secret key.");
    public_key
}

pub fn get_pair(private_key: Option<&str>) -> ic_canister_client_sender::Ed25519KeyPair {
    let contents = private_key.unwrap_or(PRIVATE_KEY);
    let (secret_key, public_key) = SecretKey::from_pem(contents).expect("Invalid secret key.");

    ic_canister_client_sender::Ed25519KeyPair {
        secret_key: secret_key.0,
        public_key: public_key.0,
    }
}

lazy_static! {
    // A keypair meant to be used in various test setups, including
    // but (not limited) to scenario tests, end-to-end tests and the
    // workload generator.
    pub static ref TEST_IDENTITY_KEYPAIR: ic_canister_client_sender::Ed25519KeyPair = {
        let mut rng = ChaChaRng::seed_from_u64(1_u64);
        ic_canister_client_sender::Ed25519KeyPair::generate(&mut rng)
    };

    // a dedicated identity for when we use --principal-id in the
    // workload generator
    pub static ref TEST_IDENTITY_KEYPAIR_HARD_CODED: ic_canister_client_sender::Ed25519KeyPair = {
        get_pair(None)
    };

    pub static ref PUBKEY : UserPublicKey = UserPublicKey {
        key: TEST_IDENTITY_KEYPAIR.public_key.to_vec(),
        algorithm_id: AlgorithmId::Ed25519,
    };

    pub static ref PUBKEY_PID : UserPublicKey = UserPublicKey {
        key: get_pub(None).as_bytes().to_vec(),
        algorithm_id: AlgorithmId::Ed25519,
    };

}
