// Test cases for Semgrep project rules in .semgrep/mailtinder.yml

fn test_mailtinder_rules() {
    // ruleid: mailtinder-no-permanent-delete
    client.delete(format!("{BASE}/users/me/messages/{id}"));

    // ruleid: mailtinder-no-permanent-delete
    let action = "batchDelete";

    // ok: mailtinder-no-permanent-delete
    client.delete(format!("{BASE}/users/me/labels/{id}"));

    // ruleid: mailtinder-no-wall-clock-or-thread-rng
    let now = SystemTime::now();

    // ok: mailtinder-no-wall-clock-or-thread-rng
    let now = clock.now();
}

fn crypto_rules() {
    // ruleid: mailtinder-crypto-approved-aead-only
    let cipher = Aes256::new(&key);

    // ruleid: mailtinder-crypto-approved-aead-only
    let cbc = Cbc::<Aes256>::new_from_slices(&key, &iv);

    // ruleid: mailtinder-crypto-approved-aead-only
    use rsa::pkcs1v15::Pkcs1v15Encrypt;

    // ruleid: mailtinder-crypto-approved-aead-only
    let stream = ChaCha20::new(&key, &nonce);

    // ok: mailtinder-crypto-approved-aead-only
    let cipher = Aes256Gcm::new(&key);

    // ruleid: mailtinder-crypto-no-weak-hash
    let h = md5::Md5::new();

    // ruleid: mailtinder-crypto-no-weak-hash
    let h = Sha1::new();

    // ok: mailtinder-crypto-no-weak-hash
    let h = Sha256::new();

    // ruleid: mailtinder-no-non-csprng-secrets
    let mut rng = SmallRng::from_entropy();

    // ruleid: mailtinder-no-non-csprng-secrets
    let mut rng = StdRng::seed_from_u64(42);

    // ok: mailtinder-no-non-csprng-secrets
    let bytes = rng.bytes32();
}

fn tls_verification() {
    // ruleid: mailtinder-tls-verification-on
    let c = Client::builder().danger_accept_invalid_certs(true).build();

    // ruleid: mailtinder-tls-verification-on
    let c = Client::builder().danger_accept_invalid_hostnames(true).build();

    // ruleid: mailtinder-tls-verification-on
    client.get(url).dangerous();

    // ruleid: mailtinder-tls-verification-on
    let p = NoCertificateVerification::new();

    // ok: mailtinder-tls-verification-on
    let c = Client::builder().https_only(true).build();
}
