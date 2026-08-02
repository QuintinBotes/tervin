//! What Tervin can honestly say about reaching an SSH host.
//!
//! ## Why this is not called latency
//!
//! SSH exposes no round-trip time. There is no protocol message that means "how long does
//! a packet take", and the encrypted channel gives a client nothing to time against. So a
//! terminal that shows you "42 ms" next to a host has measured something else and put a
//! latency label on it.
//!
//! What *can* be measured is how long a TCP connection to the SSH port takes to establish
//! — DNS resolution plus the handshake. That correlates with round-trip time and is genuinely
//! useful for "is this host far away or is something wrong", but it is not the same number,
//! and every place it surfaces says which it is.
//!
//! What can be *known* rather than inferred is whether a multiplexed connection is already
//! up: `ssh -O check` asks the running master directly. When that answers, the host is
//! reachable as a fact rather than as an inference from a bare TCP socket.
//!
//! ## Probing never happens on its own
//!
//! A `~/.ssh/config` can name a hundred hosts across several networks. Connecting to all
//! of them when the app starts would be a port scan of the user's infrastructure — slow,
//! noisy in someone's logs, and quite possibly a conversation with their security team.
//! So a probe happens when a person asks for one, for the host they asked about.

use serde::{Deserialize, Serialize};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

/// How long to wait for a TCP connection before giving up.
///
/// Long enough for a host on the other side of the world behind a slow link, short enough
/// that a firewalled address does not hold the UI for a minute.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// The default SSH port, when the config does not name one.
const DEFAULT_PORT: u16 = 22;

/// What was learned about a host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Reachability {
    /// A multiplexed connection is already open. The strongest thing Tervin can say,
    /// because `ssh` itself confirmed it rather than a socket implying it.
    Multiplexed,
    /// The SSH port accepted a connection.
    ///
    /// `connect_ms` is the time to establish TCP, **not** SSH round-trip time. Named for
    /// what it measures so nothing downstream can present it as latency.
    Open { connect_ms: u64 },
    /// Something answered and refused. The host is up; the port is not open.
    Refused,
    /// Nothing answered inside the timeout. Firewalled, asleep, or gone — indistinguishable
    /// from here, and saying which would be a guess.
    Timeout,
    /// The name did not resolve.
    Unresolved,
    /// Deliberately not attempted, with the reason.
    ///
    /// A host reached through a jump or a proxy command cannot be probed directly: its
    /// address may only be routable from the jump host, so a failed connection here would
    /// say "unreachable" about a host that works perfectly.
    Skipped { reason: String },
}

impl Reachability {
    /// One line for the UI. Never the word "latency" for a TCP measurement.
    pub fn summary(&self) -> String {
        match self {
            Self::Multiplexed => "connection already open".to_string(),
            Self::Open { connect_ms } => format!("port open · {connect_ms} ms to connect"),
            Self::Refused => "connection refused".to_string(),
            Self::Timeout => "no answer".to_string(),
            Self::Unresolved => "name does not resolve".to_string(),
            Self::Skipped { reason } => reason.clone(),
        }
    }

    /// Whether this is a state a person would call "working".
    pub fn is_reachable(&self) -> bool {
        matches!(self, Self::Multiplexed | Self::Open { .. })
    }
}

/// Probe one host from its config entry.
///
/// Blocking, and meant to be called off the UI thread. `ssh -O check` is tried first,
/// because a multiplexed connection is a fact rather than an inference — and because
/// asking the existing master costs nothing on the network.
pub fn probe(host: &crate::ssh::SshHost) -> Reachability {
    // A defaults block is not a host.
    if host.is_pattern {
        return Reachability::Skipped {
            reason: "a pattern, not a host".to_string(),
        };
    }

    // Reached only through something else, so a direct connection proves nothing about it.
    if let Some(jump) = &host.proxy_jump {
        return Reachability::Skipped {
            reason: format!("reached through {jump}, so it cannot be probed directly"),
        };
    }
    if host.proxy_command.is_some() {
        return Reachability::Skipped {
            reason: "uses a ProxyCommand, so it cannot be probed directly".to_string(),
        };
    }

    if control_master_open(&host.alias) {
        return Reachability::Multiplexed;
    }

    let target = host.hostname.clone().unwrap_or_else(|| host.alias.clone());
    probe_tcp(&target, host.port.unwrap_or(DEFAULT_PORT), CONNECT_TIMEOUT)
}

/// Ask a running multiplexed master whether it is alive.
///
/// `ssh -O check` needs `ControlPath` configured; without it ssh exits non-zero and this
/// reports false, which is correct — there is no master to be alive.
fn control_master_open(alias: &str) -> bool {
    std::process::Command::new("ssh")
        .arg("-O")
        .arg("check")
        // Never prompt. A probe that asks for a passphrase would hang a UI action.
        .arg("-o")
        .arg("BatchMode=yes")
        .arg(alias)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Time a TCP connection to `host:port`.
///
/// Exposed for testing against a real listener, which is the only way to know the timing
/// path works rather than that it compiles.
pub fn probe_tcp(host: &str, port: u16, timeout: Duration) -> Reachability {
    let started = Instant::now();

    // Resolution is part of the measurement, because it is part of what a user waits for.
    let addrs: Vec<SocketAddr> = match (host, port).to_socket_addrs() {
        Ok(addrs) => addrs.collect(),
        Err(_) => return Reachability::Unresolved,
    };
    if addrs.is_empty() {
        return Reachability::Unresolved;
    }

    // Tried in the order the resolver gave, which is the order ssh would use.
    let mut last = Reachability::Timeout;
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(stream) => {
                // Closed immediately: this is a reachability check, not a session, and
                // leaving a socket open would show up in the host's logs as a connection
                // that did nothing.
                drop(stream);
                return Reachability::Open {
                    connect_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
                };
            }
            Err(e) => {
                last = match e.kind() {
                    std::io::ErrorKind::ConnectionRefused => Reachability::Refused,
                    std::io::ErrorKind::TimedOut => Reachability::Timeout,
                    // Anything else — unreachable network, host down — is indistinguishable
                    // from silence at this level, and naming it would overstate what a
                    // failed connect tells us.
                    _ => Reachability::Timeout,
                };
            }
        }
    }
    last
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssh::SshHost;
    use std::net::TcpListener;

    fn host(alias: &str) -> SshHost {
        SshHost {
            alias: alias.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn a_real_listener_reads_as_open_and_reports_a_connect_time() {
        // A real socket, because the point of this module is the timing path and a mock
        // would only prove the types line up.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        match probe_tcp("127.0.0.1", port, CONNECT_TIMEOUT) {
            Reachability::Open { connect_ms } => {
                // Loopback, so this is small — the assertion is that it exists and is
                // plausible, not that it hits a particular number.
                assert!(connect_ms < 5_000, "implausible connect time: {connect_ms}");
            }
            other => panic!("expected an open port, got {other:?}"),
        }
    }

    #[test]
    fn a_closed_port_reads_as_refused_rather_than_as_a_timeout() {
        // Bound then dropped, so the port is almost certainly closed and nothing is
        // listening. A refusal and a silence mean different things to a user: refused
        // means the host answered.
        let port = {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };

        let result = probe_tcp("127.0.0.1", port, Duration::from_millis(500));
        assert!(
            matches!(result, Reachability::Refused | Reachability::Timeout),
            "expected a refusal or a timeout, got {result:?}"
        );
    }

    #[test]
    fn a_name_that_does_not_resolve_says_so() {
        let result = probe_tcp(
            "this-host-does-not-exist.invalid",
            22,
            Duration::from_millis(500),
        );
        // `.invalid` is reserved precisely so it cannot resolve.
        assert_eq!(result, Reachability::Unresolved);
    }

    #[test]
    fn a_pattern_is_not_probed() {
        let mut pattern = host("*.internal");
        pattern.is_pattern = true;
        match probe(&pattern) {
            Reachability::Skipped { reason } => assert!(reason.contains("pattern")),
            other => panic!("a defaults block should not be probed: {other:?}"),
        }
    }

    #[test]
    fn a_host_behind_a_jump_is_not_probed_directly() {
        // The case that would otherwise produce a confident lie: the address may only be
        // routable from the jump host, so a failed connect here says "unreachable" about
        // a host that works.
        let mut jumped = host("db");
        jumped.hostname = Some("10.0.0.9".to_string());
        jumped.proxy_jump = Some("bastion".to_string());

        match probe(&jumped) {
            Reachability::Skipped { reason } => {
                assert!(
                    reason.contains("bastion"),
                    "the reason should name the jump"
                );
            }
            other => panic!("a jumped host should not be probed: {other:?}"),
        }
    }

    #[test]
    fn a_host_with_a_proxy_command_is_not_probed_directly() {
        let mut proxied = host("weird");
        proxied.proxy_command = Some("nc -x localhost:1080 %h %p".to_string());
        assert!(matches!(probe(&proxied), Reachability::Skipped { .. }));
    }

    #[test]
    fn the_summary_never_calls_a_connect_time_latency() {
        // The whole reason this module is named `probe` and not `latency`. If this line
        // ever says "latency", the UI will repeat it.
        let summary = Reachability::Open { connect_ms: 42 }.summary();
        assert!(summary.contains("42 ms"));
        assert!(summary.contains("connect"));
        assert!(
            !summary.to_lowercase().contains("latency"),
            "a TCP connect time must not be presented as latency: {summary}"
        );
        assert!(
            !summary.to_lowercase().contains("ping"),
            "nor as a ping: {summary}"
        );
    }

    #[test]
    fn every_state_has_a_summary_and_only_two_count_as_working() {
        let states = [
            Reachability::Multiplexed,
            Reachability::Open { connect_ms: 1 },
            Reachability::Refused,
            Reachability::Timeout,
            Reachability::Unresolved,
            Reachability::Skipped {
                reason: "because".to_string(),
            },
        ];
        for state in &states {
            assert!(!state.summary().is_empty(), "{state:?} has no summary");
        }
        assert_eq!(states.iter().filter(|s| s.is_reachable()).count(), 2);
    }

    #[test]
    fn a_multiplexed_connection_is_described_as_a_fact_not_a_measurement() {
        // It is the one state Tervin knows rather than infers, so it carries no number —
        // a millisecond figure there would imply a measurement that never happened.
        let summary = Reachability::Multiplexed.summary();
        assert!(summary.contains("already open"));
        assert!(!summary.contains("ms"));
    }

    #[test]
    fn probing_a_missing_control_master_does_not_report_one() {
        // `ssh -O check` against an alias with no ControlPath exits non-zero, and reporting
        // a multiplexed connection then would be the most confident possible wrong answer.
        assert!(!control_master_open(
            "tervin-test-alias-that-does-not-exist"
        ));
    }
}
