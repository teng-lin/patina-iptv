use super::*;
use tokio::net::TcpListener;

pub(in crate::airplay) const PIN: &str = "0123";
const ACCESSORY_ID: &[u8] = b"patina-synthetic-accessory";
const SALT_BYTES: [u8; 16] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];

fn key_from_seed(seed: u8) -> Ed25519KeyPair {
    Ed25519KeyPair::from_seed_unchecked(&[seed; 32]).unwrap()
}

pub(in crate::airplay) struct Accessory {
    pub(in crate::airplay) listener: TcpListener,
    signing_key: Ed25519KeyPair,
    peer_id: &'static [u8],
}

impl Accessory {
    pub(in crate::airplay) async fn bind() -> Self {
        Self {
            listener: TcpListener::bind("127.0.0.1:0").await.unwrap(),
            signing_key: key_from_seed(9),
            peer_id: ACCESSORY_ID,
        }
    }

    pub(in crate::airplay) fn address(&self) -> SocketAddr {
        self.listener.local_addr().unwrap()
    }

    pub(in crate::airplay) async fn accept(&self) -> Wire {
        let (stream, _) = self.listener.accept().await.unwrap();
        Wire::new(stream)
    }

    pub(in crate::airplay) async fn pair(&self) -> Option<([u8; 32], Vec<u8>)> {
        let mut wire = self.accept().await;
        let prompt = wire.receive().await.unwrap();
        assert_eq!(prompt.request_target, "POST /pair-pin-start");
        wire.respond(&prompt, 200, &[]).await;

        let first = wire.receive().await.unwrap();
        assert_eq!(first.request_target, "POST /pair-setup");
        let first_fields = Fields::parse(&first.body).unwrap();
        assert_eq!(first_fields.get(METHOD).unwrap(), &[0]);
        assert_eq!(first_fields.get(STATE).unwrap(), &[1]);
        let (public, verifier, private) = server_srp();
        wire.respond(
            &first,
            200,
            &tlv(&[
                (STATE, &[2]),
                (SALT, &SALT_BYTES),
                (PUBLIC_KEY, &public.to_bytes_be()),
            ]),
        )
        .await;

        let Ok(proof) = wire.receive().await else {
            return None;
        };
        let proof_fields = Fields::parse(&proof.body).unwrap();
        assert_eq!(proof_fields.get(STATE).unwrap(), &[3]);
        let client = BigUint::from_bytes_be(proof_fields.get(PUBLIC_KEY).unwrap());
        let nonce = utils::compute_u::<Sha512>(&pad(&client), &pad(&public));
        let group = &G_3072;
        let premaster =
            ((&client * verifier.modpow(&nonce, &group.n)) % &group.n).modpow(&private, &group.n);
        let shared = digest(&[&premaster.to_bytes_be()]);
        let mut group_proof = digest(&[&group.n.to_bytes_be()]);
        for (byte, other) in group_proof
            .iter_mut()
            .zip(digest(&[&group.g.to_bytes_be()]))
        {
            *byte ^= other;
        }
        let expected = digest(&[
            &group_proof,
            &digest(&[PAIR_SETUP_USER]),
            &SALT_BYTES,
            &client.to_bytes_be(),
            &public.to_bytes_be(),
            &shared,
        ]);
        if expected != proof_fields.get(PROOF).unwrap() {
            wire.respond(&proof, 200, &tlv(&[(STATE, &[4]), (ERROR, &[2])]))
                .await;
            return None;
        }
        let own_proof = digest(&[&client.to_bytes_be(), &expected, &shared]);
        wire.respond(&proof, 200, &tlv(&[(STATE, &[4]), (PROOF, &own_proof)]))
            .await;

        let exchange = wire.receive().await.unwrap();
        let exchange_fields = Fields::parse(&exchange.body).unwrap();
        assert_eq!(exchange_fields.get(STATE).unwrap(), &[5]);
        let aead = wire::key(&derive(
            &shared,
            b"Pair-Setup-Encrypt-Salt",
            b"Pair-Setup-Encrypt-Info",
        ));
        let mut encrypted = exchange_fields.get(ENCRYPTED).unwrap().to_vec();
        let decrypted = wire::decrypt(&aead, pair_nonce(b"PS-Msg05"), b"", &mut encrypted).unwrap();
        let client_data = Fields::parse(decrypted).unwrap();
        let client_key = client_data.array(PUBLIC_KEY).unwrap();
        let client_id = client_data.get(IDENTIFIER).unwrap();
        let signing_material = derive(
            &shared,
            b"Pair-Setup-Controller-Sign-Salt",
            b"Pair-Setup-Controller-Sign-Info",
        );
        verify_signature(
            &client_key,
            &[signing_material.as_ref(), client_id, &client_key],
            client_data.get(SIGNATURE).unwrap(),
        )
        .unwrap();
        assert_eq!(client_data.get(NAME).unwrap(), b"\xe1\x44name\x46Patina");
        let signing_material = derive(
            &shared,
            b"Pair-Setup-Accessory-Sign-Salt",
            b"Pair-Setup-Accessory-Sign-Info",
        );
        let own_key = self.signing_key.public_key().as_ref();
        let signature = self
            .signing_key
            .sign(&[signing_material.as_ref(), self.peer_id, own_key].concat());
        let own_data = tlv(&[
            (IDENTIFIER, self.peer_id),
            (PUBLIC_KEY, own_key),
            (SIGNATURE, signature.as_ref()),
        ]);
        let encrypted = wire::encrypt(&aead, pair_nonce(b"PS-Msg06"), b"", own_data).unwrap();
        wire.respond(
            &exchange,
            200,
            &tlv(&[(STATE, &[6]), (ENCRYPTED, &encrypted)]),
        )
        .await;
        Some((client_key, client_id.to_vec()))
    }

    pub(in crate::airplay) async fn verify(
        &self,
        client_key: &[u8; 32],
        client_id: &[u8],
    ) -> Option<(Wire, [u8; 32])> {
        let mut wire = self.accept().await;
        let first = wire.receive().await.unwrap();
        assert_eq!(first.request_target, "POST /pair-verify");
        let fields = Fields::parse(&first.body).unwrap();
        assert_eq!(fields.get(STATE).unwrap(), &[1]);
        let client_public = fields.array::<32>(PUBLIC_KEY).unwrap();
        let private = EphemeralPrivateKey::generate(&X25519, &SystemRandom::new()).unwrap();
        let public = private.compute_public_key().unwrap();
        let shared = agreement::agree_ephemeral(
            private,
            &UnparsedPublicKey::new(&X25519, client_public),
            |secret| <[u8; 32]>::try_from(secret).unwrap(),
        )
        .unwrap();
        let aead = wire::key(&derive(
            &shared,
            b"Pair-Verify-Encrypt-Salt",
            b"Pair-Verify-Encrypt-Info",
        ));
        let peer_signature = self
            .signing_key
            .sign(&[public.as_ref(), self.peer_id, &client_public].concat());
        let own_data = tlv(&[
            (IDENTIFIER, self.peer_id),
            (SIGNATURE, peer_signature.as_ref()),
        ]);
        let encrypted = wire::encrypt(&aead, pair_nonce(b"PV-Msg02"), b"", own_data).unwrap();
        wire.respond(
            &first,
            200,
            &tlv(&[
                (STATE, &[2]),
                (PUBLIC_KEY, public.as_ref()),
                (ENCRYPTED, &encrypted),
            ]),
        )
        .await;
        let Ok(second) = wire.receive().await else {
            return None;
        };
        let fields = Fields::parse(&second.body).unwrap();
        assert_eq!(fields.get(STATE).unwrap(), &[3]);
        let mut encrypted = fields.get(ENCRYPTED).unwrap().to_vec();
        let plain = Fields::parse(
            wire::decrypt(&aead, pair_nonce(b"PV-Msg03"), b"", &mut encrypted).unwrap(),
        )
        .unwrap();
        assert_eq!(plain.get(IDENTIFIER).unwrap(), client_id);
        verify_signature(
            client_key,
            &[&client_public, client_id, public.as_ref()],
            plain.get(SIGNATURE).unwrap(),
        )
        .unwrap();
        wire.respond(&second, 200, &tlv(&[(STATE, &[4])])).await;
        wire.enable_encryption(
            &derive(&shared, b"Control-Salt", b"Control-Read-Encryption-Key"),
            &derive(&shared, b"Control-Salt", b"Control-Write-Encryption-Key"),
        );
        Some((wire, shared))
    }

    pub(in crate::airplay) fn encrypt_events(shared: &[u8; 32], wire: &mut Wire) {
        wire.enable_encryption(
            &derive(shared, b"Events-Salt", b"Events-Write-Encryption-Key"),
            &derive(shared, b"Events-Salt", b"Events-Read-Encryption-Key"),
        );
    }
}

fn server_srp() -> (BigUint, BigUint, BigUint) {
    let group = &G_3072;
    let client = SrpClient::<Sha512>::new(group);
    let verifier = BigUint::from_bytes_be(&client.compute_verifier(
        PAIR_SETUP_USER,
        PIN.as_bytes(),
        &SALT_BYTES,
    ));
    let private = BigUint::from_bytes_be(&[0xba; 32]);
    let public = (utils::compute_k::<Sha512>(group) * &verifier
        + group.g.modpow(&private, &group.n))
        % &group.n;
    (public, verifier, private)
}

#[test]
fn srp_matches_pinned_reference_for_leading_zero_pin() {
    let (public, _, _) = server_srp();
    let exchange = SrpExchange::start(
        &[0xca; 32],
        PIN.as_bytes(),
        &SALT_BYTES,
        &public.to_bytes_be(),
    )
    .unwrap();
    // Generated independently by srptools v1.0.1 with the same synthetic inputs.
    let hex = |bytes: &[u8]| {
        bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    assert_eq!(
        hex(exchange.shared.as_ref()),
        "a7019f13875993a35c54531cc28c444e344673bdb659420496de600ab0b9ddf1229eafa4980e832873e0f1f28fcfec78455f5c2ea631c5f52dba851ea98ac0b0"
    );
    assert_eq!(
        hex(&exchange.client_proof),
        "5e8eea134c4a6b12ab913f63a9968c936cd4fd44acd03a7fe603ba9997596d3874a86f80749bcda10292b3f77434153160958098545f0873fda39715ec69edd5"
    );
    assert_eq!(
        hex(&exchange.server_proof),
        "910c00f56c6941c0f2692a9498b55068f3eb624cdce24914aec227c01ff3897b320c665901e337451451049b30c0b63cc541ce50727a6284ea1a04884015c860"
    );
    assert_eq!(
        exchange.verify_server(&[]).unwrap_err(),
        AuthFault::InvalidPin
    );
    assert!(matches!(
        SrpExchange::start(&[0xca; 32], PIN.as_bytes(), &SALT_BYTES, &[0; SRP_BYTES]),
        Err(AuthFault::Wire(Fault::InvalidMessage))
    ));
}

#[test]
fn malformed_tlv_and_unexpected_states_fail_without_leaking_data() {
    let public = vec![7; 384];
    let encoded = tlv(&[(STATE, &[2]), (PUBLIC_KEY, &public)]);
    let fields = Fields::parse(&encoded)
        .unwrap()
        .require_state(2, AuthFault::ApprovalDenied)
        .unwrap();
    assert_eq!(fields.get(PUBLIC_KEY).unwrap(), public);
    assert_eq!(
        fields.get(PROOF).unwrap_err(),
        AuthFault::Wire(Fault::InvalidMessage)
    );
    for malformed in [
        vec![STATE],
        vec![STATE, 2, 0],
        vec![STATE, 1, 2, STATE, 1, 3],
    ] {
        assert!(matches!(
            Fields::parse(&malformed),
            Err(AuthFault::Wire(Fault::InvalidMessage))
        ));
    }
    for (code, error) in [
        (2, AuthFault::InvalidPin),
        (7, AuthFault::Unavailable),
        (9, AuthFault::Wire(Fault::InvalidMessage)),
    ] {
        let fields = Fields::parse(&tlv(&[(STATE, &[4]), (ERROR, &[code])])).unwrap();
        assert!(
            matches!(fields.require_state(4, AuthFault::InvalidPin), Err(actual) if actual == error)
        );
    }
    let fields = Fields::parse(&tlv(&[(STATE, &[5])])).unwrap();
    assert!(matches!(
        fields.require_state(4, AuthFault::InvalidPin),
        Err(AuthFault::Wire(Fault::InvalidMessage))
    ));
}

#[tokio::test]
async fn rejects_pin_refusal_and_preserves_identity_on_replacement() {
    let accessory = Accessory::bind().await;
    let address = accessory.address();
    let server = tokio::spawn(async move {
        assert!(accessory.pair().await.is_none());
        let (client_public, client_id) = accessory.pair().await.unwrap();
        accessory.verify(&client_public, &client_id).await.unwrap();

        let Accessory { listener, .. } = accessory;
        let replacement = Accessory {
            listener,
            signing_key: key_from_seed(3),
            peer_id: ACCESSORY_ID,
        };
        assert!(
            replacement
                .verify(&client_public, &client_id)
                .await
                .is_none()
        );
        let Accessory { listener, .. } = replacement;
        let other = Accessory {
            listener,
            signing_key: key_from_seed(9),
            peer_id: b"synthetic-replacement",
        };
        assert!(other.verify(&client_public, &client_id).await.is_none());
    });
    let challenge =
        |wire| Challenge::receive(wire, address, Instant::now() + Duration::from_secs(1));
    let first = challenge(start_pairing(address).await.unwrap())
        .await
        .unwrap();
    assert!(matches!(
        first.finish("1234").await,
        Err(AuthFailure(AuthFault::InvalidPin, Delivery::Acknowledged))
    ));
    let second = challenge(start_pairing(address).await.unwrap())
        .await
        .unwrap();
    let (verified, orignal) = second.finish(PIN).await.unwrap();
    drop(verified);
    let bytes = orignal.encode();
    let mut wrong_version = bytes.clone();
    wrong_version[0] = 2;
    for malformed in [&bytes[..20], &bytes[..bytes.len() - 1], &wrong_version] {
        assert!(matches!(
            StoredKeys::decode(malformed),
            Err(AuthFault::InvalidCredential)
        ));
    }
    let keys = StoredKeys::decode(&bytes).unwrap();
    for _ in 0..2 {
        assert!(matches!(
            verify(address, &keys).await,
            Err(AuthFailure(
                AuthFault::IdentityMismatch,
                Delivery::Acknowledged
            ))
        ));
    }
    server.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn approval_can_be_denied_or_expire() {
    let accessory = Accessory::bind().await;
    let address = accessory.address();
    let server = tokio::spawn(async move {
        let mut first = accessory.accept().await;
        let request = first.receive().await.unwrap();
        first.respond(&request, 403, &[]).await;
        let mut second = accessory.accept().await;
        let _request = second.receive().await.unwrap();
        assert!(matches!(second.receive().await, Err(Fault::Closed)));
    });
    let wire = start_pairing(address).await.unwrap();
    let denied = Challenge::receive(wire, address, Instant::now() + Duration::from_secs(15)).await;
    assert!(matches!(
        denied,
        Err(AuthFailure(
            AuthFault::ApprovalDenied,
            Delivery::Acknowledged
        ))
    ));
    let wire = start_pairing(address).await.unwrap();
    let expired = Challenge::receive(wire, address, Instant::now() + Duration::from_secs(1)).await;
    assert!(matches!(
        expired,
        Err(AuthFailure(
            AuthFault::TimedOut,
            Delivery::MayHaveReachedReceiver
        ))
    ));
    server.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn distinguishes_connect_failure_timeout_after_auth_write_and_explicit_reply() {
    let dead = Accessory::bind().await;
    let dead_address = dead.address();
    drop(dead);
    assert!(matches!(
        start_pairing(dead_address).await,
        Err(AuthFailure(
            AuthFault::Wire(Fault::Unreachable),
            Delivery::NotSent
        ))
    ));

    let accessory = Accessory::bind().await;
    let address = accessory.address();
    let server = tokio::spawn(async move {
        let mut pair = accessory.accept().await;
        let approval = pair.receive().await.unwrap();
        pair.respond(&approval, 200, &[]).await;
        let request = pair.receive().await.unwrap();
        assert_eq!(request.request_target, "POST /pair-setup");
        assert!(matches!(pair.receive().await, Err(Fault::Closed)));
        let mut verify = accessory.accept().await;
        let request = verify.receive().await.unwrap();
        assert_eq!(request.request_target, "POST /pair-verify");
        assert!(matches!(verify.receive().await, Err(Fault::Closed)));
        let mut refusal = accessory.accept().await;
        let request = refusal.receive().await.unwrap();
        refusal.respond(&request, 403, &[]).await;
    });

    let wire = start_pairing(address).await.unwrap();
    assert!(matches!(
        Challenge::receive(wire, address, Instant::now() + Duration::from_secs(15)).await,
        Err(AuthFailure(
            AuthFault::TimedOut,
            Delivery::MayHaveReachedReceiver
        ))
    ));
    let record = StoredKeys {
        seed: Zeroizing::new([5; 32]),
        peer_key: [9; 32],
        peer_id: ACCESSORY_ID.into(),
        client_id: b"synthetic-id".into(),
    };
    assert!(matches!(
        verify(address, &record).await,
        Err(AuthFailure(
            AuthFault::TimedOut,
            Delivery::MayHaveReachedReceiver
        ))
    ));
    assert!(matches!(
        verify(address, &record).await,
        Err(AuthFailure(
            AuthFault::Wire(Fault::Integrity),
            Delivery::Acknowledged
        ))
    ));
    server.await.unwrap();
}
