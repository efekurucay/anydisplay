//! Host side of the handshake (PROTOCOL.md 4) as a sans-IO state machine: feed it frames and the
//! user's SAS decision, perform the actions it returns.

use super::crypto::{self, KeyPair, SessionKeys};
use super::frame::Frame;
use super::messages::*;
use std::time::Duration;

pub const STEP_TIMEOUT: Duration = Duration::from_secs(10);
pub const USER_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    None = 0,
    Sas = 1,
    Paired = 2,
}

#[derive(Debug, Clone)]
pub struct HostIdentity {
    pub host_id: [u8; 16],
    pub host_name: String,
    pub host_os: String,
}

#[derive(Debug, Clone)]
pub struct HandshakeResult {
    pub device_id: [u8; 16],
    pub device_name: String,
    pub link: u8,
    pub mode: AuthMode,
    pub keys: SessionKeys,
    /// Mode 1: the secret from `PAIR`, to be persisted for `device_id`.
    pub pair_secret: Option<[u8; 32]>,
    pub transcript: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandshakeError {
    /// The client sent `AUTH_FAIL` with this reason.
    PeerFailed(u8),
    BadVersion,
    BadCommit,
    BadMac,
    UserRejected,
    BadPublicKey,
    Unexpected {
        channel: u8,
        ty: u8,
    },
    Malformed(String),
    Timeout,
}

impl std::fmt::Display for HandshakeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HandshakeError::PeerFailed(r) => write!(
                f,
                "client rejected the handshake: {} ({r})",
                auth_fail::name(*r)
            ),
            HandshakeError::BadVersion => {
                write!(f, "client speaks an incompatible protocol version")
            }
            HandshakeError::BadCommit => write!(f, "client nonce does not match its commitment"),
            HandshakeError::BadMac => write!(f, "client confirmation MAC is wrong"),
            HandshakeError::UserRejected => write!(f, "pairing code rejected"),
            HandshakeError::BadPublicKey => write!(f, "client public key is not a P-256 point"),
            HandshakeError::Unexpected { channel, ty } => {
                write!(
                    f,
                    "unexpected message ch={channel} type=0x{ty:02x} during handshake"
                )
            }
            HandshakeError::Malformed(m) => write!(f, "{m}"),
            HandshakeError::Timeout => write!(f, "handshake step timed out"),
        }
    }
}

impl std::error::Error for HandshakeError {}

#[derive(Debug)]
pub enum Action {
    Send(Frame),
    /// Mode 1: show this 6-digit code and report the user's answer with [`HostHandshake::user_decision`].
    ShowSas(String),
    /// Every frame after this point is encrypted, in both directions.
    EnableEncryption {
        send: [u8; 32],
        recv: [u8; 32],
    },
    Complete(HandshakeResult),
    Fail(HandshakeError),
}

struct Ctx {
    hello_c: HelloC,
    mode: AuthMode,
    psk: Vec<u8>,
    z: [u8; 32],
    transcript: [u8; 32],
}

enum State {
    AwaitHelloC,
    AwaitReveal {
        hello_c: HelloC,
        hello_c_body: Vec<u8>,
        hello_h_body: Vec<u8>,
        mode: AuthMode,
        psk: Vec<u8>,
        z: [u8; 32],
    },
    AwaitSas {
        ctx: Ctx,
        user_ok: bool,
        peer_ok: bool,
    },
    AwaitAuthC {
        ctx: Ctx,
        keys: SessionKeys,
    },
    AwaitPair {
        ctx: Ctx,
        keys: SessionKeys,
    },
    Finished,
}

/// Looks up the stored `pair_secret` of a device_id.
pub type SecretLookup = Box<dyn Fn(&[u8; 16]) -> Option<[u8; 32]> + Send>;

pub struct HostHandshake {
    identity: HostIdentity,
    key: KeyPair,
    host_nonce: [u8; 32],
    secret_for: SecretLookup,
    state: State,
}

impl HostHandshake {
    /// `secret_for` returns the stored `pair_secret` of a device, if paired.
    pub fn new(
        identity: HostIdentity,
        secret_for: impl Fn(&[u8; 16]) -> Option<[u8; 32]> + Send + 'static,
    ) -> Self {
        Self::with_key(
            identity,
            KeyPair::generate(),
            crypto::random_bytes(),
            secret_for,
        )
    }

    /// Deterministic variant for tests.
    pub fn with_key(
        identity: HostIdentity,
        key: KeyPair,
        host_nonce: [u8; 32],
        secret_for: impl Fn(&[u8; 16]) -> Option<[u8; 32]> + Send + 'static,
    ) -> Self {
        HostHandshake {
            identity,
            key,
            host_nonce,
            secret_for: Box::new(secret_for),
            state: State::AwaitHelloC,
        }
    }

    /// The device that sent `HELLO_C`, once known.
    pub fn device_id(&self) -> Option<[u8; 16]> {
        match &self.state {
            State::AwaitReveal { hello_c, .. } => Some(hello_c.device_id),
            State::AwaitSas { ctx, .. }
            | State::AwaitAuthC { ctx, .. }
            | State::AwaitPair { ctx, .. } => Some(ctx.hello_c.device_id),
            _ => None,
        }
    }

    /// How long the current step may take.
    pub fn step_timeout(&self) -> Duration {
        match self.state {
            State::AwaitSas { .. } => USER_TIMEOUT,
            _ => STEP_TIMEOUT,
        }
    }

    pub fn is_finished(&self) -> bool {
        matches!(self.state, State::Finished)
    }

    pub fn timed_out(&mut self) -> Vec<Action> {
        self.state = State::Finished;
        vec![Action::Fail(HandshakeError::Timeout)]
    }

    fn fail(&mut self, send_reason: Option<u8>, e: HandshakeError) -> Vec<Action> {
        self.state = State::Finished;
        let mut out = Vec::new();
        if let Some(reason) = send_reason {
            out.push(Action::Send(AuthFail { reason }.frame()));
        }
        out.push(Action::Fail(e));
        out
    }

    pub fn receive(&mut self, frame: &Frame) -> Vec<Action> {
        if matches!(self.state, State::Finished) {
            return vec![];
        }
        if frame.channel == channel::CONTROL && frame.ty == ty::AUTH_FAIL {
            let reason = frame.body.first().copied().unwrap_or(0);
            return self.fail(None, HandshakeError::PeerFailed(reason));
        }
        match std::mem::replace(&mut self.state, State::Finished) {
            State::AwaitHelloC => self.on_hello_c(frame),
            State::AwaitReveal {
                hello_c,
                hello_c_body,
                hello_h_body,
                mode,
                psk,
                z,
            } => {
                if !is(frame, ty::REVEAL) {
                    return self.unexpected(frame);
                }
                let reveal = match Reveal::from_body(&frame.body) {
                    Ok(r) => r,
                    Err(e) => return self.fail(None, HandshakeError::Malformed(e.to_string())),
                };
                if crypto::sha256(&[&reveal.client_nonce]) != hello_c.client_commit {
                    return self.fail(Some(auth_fail::BAD_COMMIT), HandshakeError::BadCommit);
                }
                let transcript = crypto::sha256(&[&hello_c_body, &hello_h_body, &frame.body]);
                let ctx = Ctx {
                    hello_c,
                    mode,
                    psk,
                    z,
                    transcript,
                };
                if mode == AuthMode::Sas {
                    let code = crypto::format_sas(crypto::sas_code(&transcript));
                    self.state = State::AwaitSas {
                        ctx,
                        user_ok: false,
                        peer_ok: false,
                    };
                    vec![Action::ShowSas(code)]
                } else {
                    self.send_auth_h(ctx, vec![])
                }
            }
            State::AwaitSas { ctx, user_ok, .. } => {
                if !is(frame, ty::SAS_OK) {
                    return self.unexpected(frame);
                }
                if user_ok {
                    return self.send_auth_h(ctx, vec![]);
                }
                self.state = State::AwaitSas {
                    ctx,
                    user_ok,
                    peer_ok: true,
                };
                vec![]
            }
            State::AwaitAuthC { ctx, keys } => {
                if !is(frame, ty::AUTH_C) {
                    return self.unexpected(frame);
                }
                if !crypto::verify_confirm_mac(
                    &keys.k_confirm,
                    b"client",
                    &ctx.transcript,
                    &frame.body,
                ) {
                    return self.fail(Some(auth_fail::BAD_MAC), HandshakeError::BadMac);
                }
                let mut out = Vec::new();
                if ctx.mode != AuthMode::None {
                    out.push(Action::EnableEncryption {
                        send: keys.k_h2c,
                        recv: keys.k_c2h,
                    });
                }
                if ctx.mode == AuthMode::Sas {
                    self.state = State::AwaitPair { ctx, keys };
                } else {
                    out.push(Action::Complete(result(ctx, keys, None)));
                }
                out
            }
            State::AwaitPair { ctx, keys } => {
                if !is(frame, ty::PAIR) {
                    return self.unexpected(frame);
                }
                match Pair::from_body(&frame.body) {
                    Ok(p) => vec![Action::Complete(result(ctx, keys, Some(p.pair_secret)))],
                    Err(e) => self.fail(None, HandshakeError::Malformed(e.to_string())),
                }
            }
            State::Finished => vec![],
        }
    }

    /// Mode 1: the user compared the codes.
    pub fn user_decision(&mut self, confirmed: bool) -> Vec<Action> {
        let State::AwaitSas {
            ctx,
            user_ok: false,
            peer_ok,
        } = std::mem::replace(&mut self.state, State::Finished)
        else {
            return vec![];
        };
        if !confirmed {
            return self.fail(Some(auth_fail::USER_REJECTED), HandshakeError::UserRejected);
        }
        let sas_ok = Action::Send(SasOk {}.frame());
        if peer_ok {
            self.send_auth_h(ctx, vec![sas_ok])
        } else {
            self.state = State::AwaitSas {
                ctx,
                user_ok: true,
                peer_ok: false,
            };
            vec![sas_ok]
        }
    }

    fn unexpected(&mut self, frame: &Frame) -> Vec<Action> {
        self.fail(
            None,
            HandshakeError::Unexpected {
                channel: frame.channel,
                ty: frame.ty,
            },
        )
    }

    fn on_hello_c(&mut self, frame: &Frame) -> Vec<Action> {
        if !is(frame, ty::HELLO_C) {
            return self.unexpected(frame);
        }
        // Version first: a future major may change the rest of the layout.
        if frame.body.len() < 6
            || frame.body[..4] != MAGIC
            || u16::from_le_bytes([frame.body[4], frame.body[5]]) != MAJOR
        {
            return self.fail(Some(auth_fail::BAD_VERSION), HandshakeError::BadVersion);
        }
        let hello_c = match HelloC::from_body(&frame.body) {
            Ok(h) => h,
            Err(e) => return self.fail(None, HandshakeError::Malformed(e.to_string())),
        };
        let Ok(z) = self.key.ecdh(&hello_c.client_pub) else {
            return self.fail(None, HandshakeError::BadPublicKey);
        };
        let secret = (self.secret_for)(&hello_c.device_id);
        let mode = match (secret, hello_c.auth_required) {
            (Some(_), _) => AuthMode::Paired,
            (None, 0) => AuthMode::None,
            (None, _) => AuthMode::Sas,
        };
        let hello_h = HelloH {
            magic: MAGIC,
            major: MAJOR,
            minor: MINOR,
            host_id: self.identity.host_id,
            host_name: self.identity.host_name.clone(),
            host_os: self.identity.host_os.clone(),
            auth_mode: mode as u8,
            host_pub: self.key.public,
            host_nonce: self.host_nonce,
        };
        let hello_h_frame = hello_h.frame();
        self.state = State::AwaitReveal {
            hello_c,
            hello_c_body: frame.body.clone(),
            hello_h_body: hello_h_frame.body.clone(),
            mode,
            psk: secret.map(|s| s.to_vec()).unwrap_or_default(),
            z,
        };
        vec![Action::Send(hello_h_frame)]
    }

    fn send_auth_h(&mut self, ctx: Ctx, mut out: Vec<Action>) -> Vec<Action> {
        let keys = crypto::key_schedule(&ctx.z, &ctx.psk, &ctx.transcript);
        let mac = crypto::confirm_mac(&keys.k_confirm, b"host", &ctx.transcript);
        out.push(Action::Send(AuthH { mac }.frame()));
        self.state = State::AwaitAuthC { ctx, keys };
        out
    }
}

fn is(frame: &Frame, t: u8) -> bool {
    frame.channel == channel::CONTROL && frame.ty == t
}

fn result(ctx: Ctx, keys: SessionKeys, pair_secret: Option<[u8; 32]>) -> HandshakeResult {
    HandshakeResult {
        device_id: ctx.hello_c.device_id,
        device_name: ctx.hello_c.device_name,
        link: ctx.hello_c.link,
        mode: ctx.mode,
        keys,
        pair_secret,
        transcript: ctx.transcript,
    }
}
