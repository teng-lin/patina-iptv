//! PIN-approved AirPlay pair-setup and identity-bound pair-verify.
use super::wire::{self, Fault, Version, Wire};
use crate::sender::Delivery;
use futures_util::{StreamExt, stream};
use num_bigint::BigUint;
use ring::{
    agreement::{self, EphemeralPrivateKey, UnparsedPublicKey, X25519},
    hkdf::{self, HKDF_SHA512},
    rand::{SecureRandom, SystemRandom},
    signature::{ED25519, Ed25519KeyPair, KeyPair as _, UnparsedPublicKey as SigningPublicKey},
};
use sha2::{Digest as _, Sha512};
use srp::{client::SrpClient, groups::G_3072, utils};
use std::{collections::BTreeMap, net::SocketAddr, time::Duration};
use subtle::ConstantTimeEq as _;
use tokio::time::{Instant, timeout, timeout_at};
use zeroize::Zeroizing;

const AUTH_HEADERS: &[(&str, &str)] = &[
    ("User-Agent", "AirPlay/870.14.1"),
    ("Connection", "keep-alive"),
    ("X-Apple-HKP", "3"),
    ("Content-Type", "application/octet-stream"),
];

const METHOD: u8 = 0;
const IDENTIFIER: u8 = 1;
const SALT: u8 = 2;
const PUBLIC_KEY: u8 = 3;
const PROOF: u8 = 4;
const ENCRYPTED: u8 = 5;
const STATE: u8 = 6;
const ERROR: u8 = 7;
const SIGNATURE: u8 = 10;
const NAME: u8 = 17;
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(5);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const PAIR_SETUP_USER: &[u8] = b"Pair-Setup";
const SRP_BYTES: usize = 384;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AuthFault {
    Wire(Fault),
    TimedOut,
    ApprovalDenied,
    InvalidPin,
    IdentityMismatch,
    InvalidCredential,
    Unavailable,
}

impl From<Fault> for AuthFault {
    fn from(error: Fault) -> Self {
        Self::Wire(error)
    }
}

type Result<T> = std::result::Result<T, AuthFault>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct AuthFailure(pub(super) AuthFault, pub(super) Delivery);

impl From<AuthFault> for AuthFailure {
    fn from(fault: AuthFault) -> Self {
        Self(fault, Delivery::NotSent)
    }
}

impl From<Fault> for AuthFailure {
    fn from(fault: Fault) -> Self {
        AuthFault::from(fault).into()
    }
}

type Reported<T> = std::result::Result<T, AuthFailure>;

fn acknowledged<T>(response: Result<T>) -> Reported<T> {
    response.map_err(|fault| AuthFailure(fault, Delivery::Acknowledged))
}

fn in_flight(fault: impl Into<AuthFault>) -> AuthFailure {
    AuthFailure(fault.into(), Delivery::MayHaveReachedReceiver)
}

fn pair_nonce(label: &[u8; 8]) -> [u8; 12] {
    let mut nonce = [0; 12];
    nonce[4..].copy_from_slice(label);
    nonce
}

fn random<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0; N];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| AuthFault::Unavailable)?;
    Ok(bytes)
}

pub(super) fn uuid() -> Result<String> {
    let mut bytes = random::<16>()?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02X}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

fn digest(fields: &[&[u8]]) -> [u8; 64] {
    let mut hash = Sha512::new();
    for field in fields {
        hash.update(field);
    }
    hash.finalize().into()
}

struct KeyLength;
impl hkdf::KeyType for KeyLength {
    fn len(&self) -> usize {
        32
    }
}

fn derive(shared: &[u8], salt: &[u8], info: &[u8]) -> Zeroizing<[u8; 32]> {
    let mut result = Zeroizing::new([0; 32]);
    hkdf::Salt::new(HKDF_SHA512, salt)
        .extract(shared)
        .expand(&[info], KeyLength)
        .expect("fixed HKDF output length")
        .fill(result.as_mut())
        .expect("output length matches key length");
    result
}

fn tlv(fields: &[(u8, &[u8])]) -> Vec<u8> {
    let mut encoded = Vec::new();
    for (kind, field) in fields {
        for chunk in field.chunks(255) {
            encoded.extend([*kind, chunk.len() as u8]);
            encoded.extend_from_slice(chunk);
        }
    }
    encoded
}

struct Fields(BTreeMap<u8, Vec<u8>>);

impl Fields {
    fn parse(mut bytes: &[u8]) -> Result<Self> {
        let mut fields = BTreeMap::<u8, Vec<u8>>::new();
        let mut previous = None;
        while !bytes.is_empty() {
            let [kind, len] = *bytes.first_chunk::<2>().ok_or(Fault::InvalidMessage)?;
            let (chunk, rest) = bytes[2..]
                .split_at_checked(usize::from(len))
                .ok_or(Fault::InvalidMessage)?;
            if fields.contains_key(&kind) && previous != Some((kind, u8::MAX)) {
                return Err(Fault::InvalidMessage.into());
            }
            fields.entry(kind).or_default().extend_from_slice(chunk);
            previous = Some((kind, len));
            bytes = rest;
        }
        Ok(Self(fields))
    }

    fn get(&self, tag: u8) -> Result<&[u8]> {
        self.0
            .get(&tag)
            .filter(|field| !field.is_empty())
            .map(Vec::as_slice)
            .ok_or(Fault::InvalidMessage.into())
    }

    fn array<const N: usize>(&self, tag: u8) -> Result<[u8; N]> {
        self.get(tag)?
            .try_into()
            .map_err(|_| Fault::InvalidMessage.into())
    }

    fn require_state(self, at: u8, authentication_error: AuthFault) -> Result<Self> {
        if self.array::<1>(STATE)? != [at] {
            return Err(Fault::InvalidMessage.into());
        }
        if let Some(reason) = self.0.get(&ERROR) {
            return Err(match reason.as_slice() {
                [2] => authentication_error,
                [3..=7] => AuthFault::Unavailable,
                _ => Fault::InvalidMessage.into(),
            });
        }
        Ok(self)
    }
}

pub(super) struct StoredKeys {
    seed: Zeroizing<[u8; 32]>,
    peer_key: [u8; 32],
    peer_id: Vec<u8>,
    client_id: Vec<u8>,
}

impl StoredKeys {
    pub(super) fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(67 + self.peer_id.len() + self.client_id.len());
        bytes.push(1);
        bytes.extend_from_slice(self.seed.as_ref());
        bytes.extend(self.peer_key);
        bytes.extend([self.peer_id.len() as u8, self.client_id.len() as u8]);
        bytes.extend_from_slice(&self.peer_id);
        bytes.extend_from_slice(&self.client_id);
        bytes
    }

    pub(super) fn decode(bytes: &[u8]) -> Result<Self> {
        let Some((fixed, ids)) = bytes.split_first_chunk::<67>() else {
            return Err(AuthFault::InvalidCredential);
        };
        let (peer_len, client_len) = (usize::from(fixed[65]), usize::from(fixed[66]));
        if fixed[0] != 1 || peer_len == 0 || client_len == 0 || ids.len() != peer_len + client_len {
            return Err(AuthFault::InvalidCredential);
        }
        Ok(Self {
            seed: Zeroizing::new(fixed[1..33].try_into().expect("32-byte signing seed")),
            peer_key: fixed[33..65].try_into().expect("32-byte peer key"),
            peer_id: ids[..peer_len].to_vec(),
            client_id: ids[peer_len..].to_vec(),
        })
    }

    fn signing_key(&self) -> Result<Ed25519KeyPair> {
        Ed25519KeyPair::from_seed_unchecked(self.seed.as_ref())
            .map_err(|_| AuthFault::InvalidCredential)
    }
}

fn pad(number: &BigUint) -> [u8; SRP_BYTES] {
    let bytes = number.to_bytes_be();
    let mut padded = [0; SRP_BYTES];
    padded[SRP_BYTES - bytes.len()..].copy_from_slice(&bytes);
    padded
}

struct SrpExchange {
    public: Vec<u8>,
    client_proof: [u8; 64],
    server_proof: [u8; 64],
    shared: Zeroizing<[u8; 64]>,
}

impl SrpExchange {
    fn start(secret: &[u8; 32], pin: &[u8], salt: &[u8; 16], remote_key: &[u8]) -> Result<Self> {
        let group = &G_3072;
        let remote = BigUint::from_bytes_be(remote_key);
        if remote == BigUint::default() || remote >= group.n {
            return Err(Fault::InvalidMessage.into());
        }
        let client = SrpClient::<Sha512>::new(group);
        let secret = BigUint::from_bytes_be(secret);
        let public = client.compute_a_pub(&secret);
        let nonce = utils::compute_u::<Sha512>(&pad(&public), &pad(&remote));
        if nonce == BigUint::default() {
            return Err(Fault::InvalidMessage.into());
        }
        let identity = SrpClient::<Sha512>::compute_identity_hash(PAIR_SETUP_USER, pin);
        let password = SrpClient::<Sha512>::compute_x(&identity, salt);
        let premaster = client.compute_premaster_secret(
            &remote,
            &utils::compute_k::<Sha512>(group),
            &password,
            &secret,
            &nonce,
        );
        if premaster == BigUint::default() {
            return Err(Fault::InvalidMessage.into());
        }
        let shared = Zeroizing::new(digest(&[&premaster.to_bytes_be()]));

        // AirPlay uses the original SRP proof, which differs from the short
        // proof in RustCrypto's process_reply. Only k and u use padded inputs.
        let mut group_proof = digest(&[&group.n.to_bytes_be()]);
        for (byte, other) in group_proof
            .iter_mut()
            .zip(digest(&[&group.g.to_bytes_be()]))
        {
            *byte ^= other;
        }
        let public = public.to_bytes_be();
        let client_proof = digest(&[
            &group_proof,
            &digest(&[PAIR_SETUP_USER]),
            salt,
            &public,
            &remote.to_bytes_be(),
            shared.as_ref(),
        ]);
        let server_proof = digest(&[&public, &client_proof, shared.as_ref()]);
        Ok(Self {
            public,
            client_proof,
            server_proof,
            shared,
        })
    }

    fn verify_server(&self, received: &[u8]) -> Result<()> {
        if bool::from(self.server_proof.ct_eq(received)) {
            Ok(())
        } else {
            Err(AuthFault::InvalidPin)
        }
    }
}

async fn auth_request(
    wire: &mut Wire,
    path: &str,
    data: &[u8],
    authentication_error: AuthFault,
    expected_state: u8,
) -> Reported<Fields> {
    let response = timeout(
        HANDSHAKE_TIMEOUT,
        wire.request(Version::Http, "POST", path, AUTH_HEADERS, data),
    )
    .await
    .map_err(|_| in_flight(AuthFault::TimedOut))?
    .map_err(in_flight)?;
    match response.require_ok() {
        Err(Fault::Refused(401 | 403 | 470)) => acknowledged(Err(authentication_error)),
        Err(error) => acknowledged(Err(error.into())),
        Ok(response) => acknowledged(
            Fields::parse(&response.body)
                .and_then(|fields| fields.require_state(expected_state, authentication_error)),
        ),
    }
}

async fn open(address: SocketAddr) -> Result<Wire> {
    Ok(timeout(CONNECTION_TIMEOUT, Wire::connect(address))
        .await
        .map_err(|_| AuthFault::TimedOut)??)
}

pub(super) async fn open_candidates(addresses: &[SocketAddr]) -> Result<(SocketAddr, Wire)> {
    let attempts = stream::iter(addresses.iter().copied().map(|address| async move {
        Wire::connect(address)
            .await
            .ok()
            .map(|wire| (address, wire))
    }))
    .buffer_unordered(4)
    .filter_map(std::future::ready);
    tokio::pin!(attempts);
    timeout(CONNECTION_TIMEOUT, attempts.next())
        .await
        .map_err(|_| AuthFault::TimedOut)?
        .ok_or_else(|| Fault::Unreachable.into())
}

pub(super) struct Challenge {
    wire: Wire,
    address: SocketAddr,
    public: Vec<u8>,
    salt: [u8; 16],
}

#[cfg(test)]
pub(super) async fn start_pairing(address: SocketAddr) -> Reported<Wire> {
    start_pairing_on_wire(open(address).await?).await
}

pub(super) async fn start_pairing_on_wire(mut wire: Wire) -> Reported<Wire> {
    // The receiver may wait for approval before replying. Return after the
    // request is sent so the caller can display the pending approval prompt.
    timeout(
        HANDSHAKE_TIMEOUT,
        wire.send_request(Version::Http, "POST", "/pair-pin-start", AUTH_HEADERS, &[]),
    )
    .await
    .map_err(|_| in_flight(AuthFault::TimedOut))?
    .map_err(in_flight)?;
    Ok(wire)
}

impl Challenge {
    pub(super) async fn receive(
        mut wire: Wire,
        address: SocketAddr,
        deadline: Instant,
    ) -> Reported<Self> {
        let response = timeout_at(deadline, wire.receive_response(Version::Http))
            .await
            .map_err(|_| in_flight(AuthFault::TimedOut))?
            .map_err(in_flight)?;
        match response.require_ok() {
            Err(Fault::Refused(401 | 403 | 470)) => {
                return acknowledged(Err(AuthFault::ApprovalDenied));
            }
            Err(error) => return acknowledged(Err(error.into())),
            Ok(_) => {}
        }
        let request = tlv(&[(METHOD, &[0]), (STATE, &[1])]);
        let fields = auth_request(
            &mut wire,
            "/pair-setup",
            &request,
            AuthFault::ApprovalDenied,
            2,
        )
        .await?;
        let salt = acknowledged(fields.array(SALT))?;
        let public = acknowledged(fields.get(PUBLIC_KEY))?;
        if public.len() > SRP_BYTES {
            return acknowledged(Err(Fault::InvalidMessage.into()));
        }
        Ok(Self {
            wire,
            address,
            public: public.to_vec(),
            salt,
        })
    }

    pub(super) async fn finish(mut self, pin: &str) -> Reported<(Verified, StoredKeys)> {
        if pin.len() != 4 || !pin.bytes().all(|digit| digit.is_ascii_digit()) {
            return Err(AuthFault::InvalidPin.into());
        }
        let srp_secret = Zeroizing::new(random::<32>()?);
        let exchange = SrpExchange::start(&srp_secret, pin.as_bytes(), &self.salt, &self.public)?;

        let request = tlv(&[
            (STATE, &[3]),
            (PUBLIC_KEY, &exchange.public),
            (PROOF, &exchange.client_proof),
        ]);
        let response = auth_request(
            &mut self.wire,
            "/pair-setup",
            &request,
            AuthFault::InvalidPin,
            4,
        )
        .await?;
        acknowledged(
            response
                .get(PROOF)
                .and_then(|proof| exchange.verify_server(proof)),
        )?;

        let seed = Zeroizing::new(random::<32>()?);
        let signing = Ed25519KeyPair::from_seed_unchecked(seed.as_ref())
            .map_err(|_| AuthFault::Unavailable)?;
        let client_id = uuid()?.into_bytes();
        let signing_material = derive(
            exchange.shared.as_ref(),
            b"Pair-Setup-Controller-Sign-Salt",
            b"Pair-Setup-Controller-Sign-Info",
        );
        let client_public = signing.public_key().as_ref();
        let signature =
            signing.sign(&[signing_material.as_ref(), &client_id, client_public].concat());
        // OPACK dictionary with one entry, "name": "Patina". The receiver
        // displays this name in its paired-client list.
        const DISPLAY_NAME: &[u8] = b"\xe1\x44name\x46Patina";
        let sub = tlv(&[
            (IDENTIFIER, &client_id),
            (PUBLIC_KEY, client_public),
            (SIGNATURE, signature.as_ref()),
            (NAME, DISPLAY_NAME),
        ]);
        let aead = wire::key(&derive(
            exchange.shared.as_ref(),
            b"Pair-Setup-Encrypt-Salt",
            b"Pair-Setup-Encrypt-Info",
        ));
        let ciphertext = wire::encrypt(&aead, pair_nonce(b"PS-Msg05"), b"", sub)?;
        let response = auth_request(
            &mut self.wire,
            "/pair-setup",
            &tlv(&[(STATE, &[5]), (ENCRYPTED, &ciphertext)]),
            AuthFault::InvalidPin,
            6,
        )
        .await?;
        let mut ciphertext = acknowledged(response.get(ENCRYPTED))?.to_vec();
        let plain = acknowledged(
            wire::decrypt(&aead, pair_nonce(b"PS-Msg06"), b"", &mut ciphertext).map_err(Into::into),
        )?;
        let fields = acknowledged(Fields::parse(plain))?;

        let peer_key = acknowledged(fields.array::<32>(PUBLIC_KEY))?;
        let peer_id = acknowledged(fields.get(IDENTIFIER))?;
        if peer_id.len() > u8::MAX.into() {
            return acknowledged(Err(Fault::InvalidMessage.into()));
        }
        let signing_material = derive(
            exchange.shared.as_ref(),
            b"Pair-Setup-Accessory-Sign-Salt",
            b"Pair-Setup-Accessory-Sign-Info",
        );
        acknowledged(verify_signature(
            &peer_key,
            &[signing_material.as_ref(), peer_id, &peer_key],
            acknowledged(fields.get(SIGNATURE))?,
        ))?;
        let keys = StoredKeys {
            seed,
            peer_key,
            peer_id: peer_id.to_vec(),
            client_id,
        };

        // Retire the approval connection. Persist only a record successfully
        // verified against a fresh encrypted playback connection.
        drop(self.wire);
        let verified = verify(self.address, &keys)
            .await
            .map_err(|AuthFailure(fault, _)| AuthFailure(fault, Delivery::Acknowledged))?;
        Ok((verified, keys))
    }
}

pub(super) struct Verified {
    pub(super) wire: Wire,
    shared: Zeroizing<[u8; 32]>,
    pub(super) device_id: String,
}

impl Verified {
    pub(super) fn encrypt_events(&self, wire: &mut Wire) {
        // This is a reverse channel: the receiver initiates requests.
        wire.enable_encryption(
            &derive(
                self.shared.as_ref(),
                b"Events-Salt",
                b"Events-Read-Encryption-Key",
            ),
            &derive(
                self.shared.as_ref(),
                b"Events-Salt",
                b"Events-Write-Encryption-Key",
            ),
        );
    }
}

fn verify_signature(key: &[u8; 32], message: &[&[u8]], signature: &[u8]) -> Result<()> {
    SigningPublicKey::new(&ED25519, key)
        .verify(&message.concat(), signature)
        .map_err(|_| AuthFault::IdentityMismatch)
}

pub(super) async fn verify(address: SocketAddr, keys: &StoredKeys) -> Reported<Verified> {
    verify_on_wire(open(address).await?, keys).await
}

pub(super) async fn verify_on_wire(mut wire: Wire, keys: &StoredKeys) -> Reported<Verified> {
    let private = EphemeralPrivateKey::generate(&X25519, &SystemRandom::new())
        .map_err(|_| AuthFault::Unavailable)?;
    let public = private
        .compute_public_key()
        .map_err(|_| AuthFault::Unavailable)?;
    let response = auth_request(
        &mut wire,
        "/pair-verify",
        &tlv(&[(STATE, &[1]), (PUBLIC_KEY, public.as_ref())]),
        Fault::Integrity.into(),
        2,
    )
    .await?;
    let peer_public = acknowledged(response.array::<32>(PUBLIC_KEY))?;
    let shared = acknowledged(
        agreement::agree_ephemeral(
            private,
            &UnparsedPublicKey::new(&X25519, peer_public),
            |secret| Zeroizing::new(<[u8; 32]>::try_from(secret).expect("X25519 output length")),
        )
        .map_err(|_| Fault::Integrity.into()),
    )?;
    let aead = wire::key(&derive(
        shared.as_ref(),
        b"Pair-Verify-Encrypt-Salt",
        b"Pair-Verify-Encrypt-Info",
    ));
    let mut ciphertext = acknowledged(response.get(ENCRYPTED))?.to_vec();
    let plain = acknowledged(
        wire::decrypt(&aead, pair_nonce(b"PV-Msg02"), b"", &mut ciphertext).map_err(Into::into),
    )?;
    let fields = acknowledged(Fields::parse(plain))?;
    let peer_id = acknowledged(fields.get(IDENTIFIER))?;
    if peer_id != keys.peer_id {
        return acknowledged(Err(AuthFault::IdentityMismatch));
    }
    acknowledged(verify_signature(
        &keys.peer_key,
        &[&peer_public, peer_id, public.as_ref()],
        acknowledged(fields.get(SIGNATURE))?,
    ))?;

    let key = keys.signing_key()?;
    let signature = key.sign(&[public.as_ref(), &keys.client_id, &peer_public].concat());
    let sub = tlv(&[
        (IDENTIFIER, &keys.client_id),
        (SIGNATURE, signature.as_ref()),
    ]);
    let ciphertext = wire::encrypt(&aead, pair_nonce(b"PV-Msg03"), b"", sub)?;
    auth_request(
        &mut wire,
        "/pair-verify",
        &tlv(&[(STATE, &[3]), (ENCRYPTED, &ciphertext)]),
        AuthFault::InvalidCredential,
        4,
    )
    .await?;

    wire.enable_encryption(
        &derive(
            shared.as_ref(),
            b"Control-Salt",
            b"Control-Write-Encryption-Key",
        ),
        &derive(
            shared.as_ref(),
            b"Control-Salt",
            b"Control-Read-Encryption-Key",
        ),
    );
    let mut device = key.public_key().as_ref()[..6].to_vec();
    // A stable locally administered unicast address without exposing host identifiers.
    device[0] = (device[0] & !1) | 2;
    let device_id = device
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(":");
    Ok(Verified {
        wire,
        shared,
        device_id,
    })
}

#[cfg(test)]
pub(super) mod tests;
