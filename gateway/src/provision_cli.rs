use std::io::{ErrorKind, Read};

use anyhow::{bail, Context, Result};
use wow_srp::normalized_string::{NormalizedString, MAXIMUM_STRING_LENGTH_IN_BYTES};
use zeroize::Zeroizing;

pub(crate) const PROVISION_USAGE: &str =
    "usage: gateway provision <USERNAME> --password-stdin [--create-only]";
const MAX_PASSWORD_BYTES: usize = MAXIMUM_STRING_LENGTH_IN_BYTES as usize;
const MAX_PASSWORD_STDIN_BYTES: usize = MAX_PASSWORD_BYTES + 2;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum GatewayMode {
    Serve,
    Provision { username: String, create_only: bool },
}

pub(crate) fn parse_gateway_mode(mut args: impl Iterator<Item = String>) -> Result<GatewayMode> {
    let Some(command) = args.next() else {
        return Ok(GatewayMode::Serve);
    };

    if command != "provision" {
        bail!("usage: gateway [provision <USERNAME> --password-stdin [--create-only]]");
    }

    let Some(username) = args.next() else {
        bail!("{PROVISION_USAGE}");
    };
    let mut password_stdin = false;
    let mut create_only = false;
    for argument in args {
        match argument.as_str() {
            "--password-stdin" if !password_stdin => password_stdin = true,
            "--create-only" if !create_only => create_only = true,
            // An argument may be a plaintext password; never include it in diagnostics.
            _ => bail!("{PROVISION_USAGE}"),
        }
    }
    if !password_stdin {
        bail!("{PROVISION_USAGE}");
    }

    Ok(GatewayMode::Provision {
        username,
        create_only,
    })
}

pub(crate) fn read_password_line(reader: &mut impl Read) -> Result<Zeroizing<Vec<u8>>> {
    // Reserve before reading secrets; Vec growth can free an allocation before Zeroizing wipes it.
    let mut password = Zeroizing::new(Vec::with_capacity(MAX_PASSWORD_STDIN_BYTES));
    let mut byte = Zeroizing::new([0_u8; 1]);
    let mut bytes_read = 0;
    let mut ended_with_lf = false;

    loop {
        if bytes_read == MAX_PASSWORD_STDIN_BYTES {
            bail!("invalid password: input exceeds {MAX_PASSWORD_BYTES} bytes");
        }

        let count = match reader.read(&mut *byte) {
            Ok(count) => count,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(error).context("failed to read password from stdin"),
        };
        if count == 0 {
            break;
        }
        bytes_read += 1;

        if byte[0] == b'\n' {
            ended_with_lf = true;
            break;
        }
        password.push(byte[0]);
    }

    if ended_with_lf && password.last() == Some(&b'\r') {
        // Clear CR before pop; Zeroizing wipes only the initialized slice.
        *password.last_mut().expect("last byte was just checked") = 0;
        password.pop();
    }
    if password.len() > MAX_PASSWORD_BYTES {
        bail!("invalid password: input exceeds {MAX_PASSWORD_BYTES} bytes");
    }

    Ok(password)
}

pub(crate) fn normalize_provision_credentials(
    username: &str,
    password: &[u8],
) -> Result<(NormalizedString, NormalizedString)> {
    let username = NormalizedString::new(username)
        .map_err(|error| anyhow::anyhow!("invalid username: {error:?}"))?;
    let password = std::str::from_utf8(password)
        .map_err(|_| anyhow::anyhow!("invalid password: expected 1-16 non-control ASCII bytes"))?;
    let password = NormalizedString::new(password)
        .map_err(|_| anyhow::anyhow!("invalid password: expected 1-16 non-control ASCII bytes"))?;
    Ok((username, password))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn parse(args: &[&str]) -> Result<GatewayMode> {
        parse_gateway_mode(args.iter().map(|argument| (*argument).to_owned()))
    }

    #[test]
    fn no_arguments_starts_the_gateway() {
        assert_eq!(parse(&[]).unwrap(), GatewayMode::Serve);
    }

    #[test]
    fn provision_accepts_only_the_password_stdin_form() {
        assert_eq!(
            parse(&["provision", "alice", "--password-stdin"]).unwrap(),
            GatewayMode::Provision {
                username: "alice".to_owned(),
                create_only: false,
            }
        );
    }

    #[test]
    fn create_only_accepts_both_flag_orders() {
        for args in [
            ["provision", "alice", "--password-stdin", "--create-only"],
            ["provision", "alice", "--create-only", "--password-stdin"],
        ] {
            assert_eq!(
                parse(&args).unwrap(),
                GatewayMode::Provision {
                    username: "alice".to_owned(),
                    create_only: true,
                }
            );
        }
    }

    #[test]
    fn positional_password_is_rejected_without_echoing_it() {
        let secret = "do-not-print-this";
        let error = parse(&["provision", "alice", secret])
            .unwrap_err()
            .to_string();
        assert_eq!(error, PROVISION_USAGE);
        assert!(!error.contains(secret));
    }

    #[test]
    fn provision_rejects_missing_or_extra_arguments() {
        for args in [
            &["provision"][..],
            &["provision", "alice"][..],
            &["provision", "alice", "--password-stdin", "extra"][..],
            &["provision", "alice", "--create-only"][..],
            &["provision", "alice", "--password-stdin", "--password-stdin"][..],
            &[
                "provision",
                "alice",
                "--password-stdin",
                "--create-only",
                "--create-only",
            ][..],
        ] {
            assert_eq!(parse(args).unwrap_err().to_string(), PROVISION_USAGE);
        }
    }

    #[test]
    fn password_reader_removes_lf_and_crlf_only() {
        let mut lf = Cursor::new(&b" pass phrase \nignored"[..]);
        assert_eq!(&*read_password_line(&mut lf).unwrap(), b" pass phrase ");

        let mut crlf = Cursor::new(&b"password\r\n"[..]);
        assert_eq!(&*read_password_line(&mut crlf).unwrap(), b"password");

        let mut bare_cr = Cursor::new(&b"password\r"[..]);
        assert_eq!(&*read_password_line(&mut bare_cr).unwrap(), b"password\r");

        let mut eof = Cursor::new(&b"password"[..]);
        assert_eq!(&*read_password_line(&mut eof).unwrap(), b"password");
    }

    #[test]
    fn password_reader_accepts_the_maximum_length_with_crlf() {
        let mut input = Cursor::new(&b"1234567890abcdef\r\n"[..]);
        assert_eq!(
            &*read_password_line(&mut input).unwrap(),
            b"1234567890abcdef"
        );
    }

    #[test]
    fn password_reader_rejects_overlong_input_without_echoing_it() {
        let secret = b"1234567890abcdefg\n";
        let mut input = Cursor::new(&secret[..]);
        let error = read_password_line(&mut input).unwrap_err().to_string();
        assert!(error.contains("exceeds 16 bytes"));
        assert!(!error.contains(std::str::from_utf8(secret).unwrap().trim_end()));
    }

    #[test]
    fn normalized_credentials_canonicalize_username_and_password() {
        let (username, password) = normalize_provision_credentials("alice", b"pass word").unwrap();
        assert_eq!(username.as_ref(), "ALICE");
        assert_eq!(password.as_ref(), "PASS WORD");
    }

    #[test]
    fn an_interrupted_read_is_retried_without_losing_or_duplicating_a_byte() {
        struct Flaky {
            remaining: Vec<u8>,
            interrupt_next: bool,
        }
        impl Read for Flaky {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if self.interrupt_next {
                    self.interrupt_next = false;
                    return Err(std::io::Error::new(ErrorKind::Interrupted, "signal"));
                }
                self.interrupt_next = true;
                if self.remaining.is_empty() {
                    return Ok(0);
                }
                buf[0] = self.remaining.remove(0);
                Ok(1)
            }
        }

        let mut reader = Flaky {
            remaining: b"pass word\n".to_vec(),
            interrupt_next: true,
        };
        assert_eq!(
            &*read_password_line(&mut reader).unwrap(),
            b"pass word",
            "an EINTR must be retried transparently — dropping the interrupted byte silently \
             provisions a DIFFERENT password than the operator typed, and nothing reports it until \
             the first login fails"
        );
    }

    #[test]
    fn a_real_read_error_fails_instead_of_retrying_forever() {
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::new(ErrorKind::BrokenPipe, "gone"))
            }
        }
        let error = read_password_line(&mut Broken).unwrap_err().to_string();
        assert!(
            error.contains("failed to read password from stdin"),
            "a broken stdin must be reported, not retried: {error}"
        );
    }

    #[test]
    fn any_stdin_byte_stream_is_read_within_the_advertised_bound() {
        let mut rng = crate::codec::property_tests::Rng::new(0x5354_4449_4E00);
        for _ in 0..2_000 {
            let len = rng.below(40);
            let mut input: Vec<u8> = rng.bytes(len);
            for byte in input.iter_mut() {
                match rng.below(8) {
                    0 => *byte = b'\n',
                    1 => *byte = b'\r',
                    _ => {}
                }
            }

            let mut cursor = Cursor::new(input.clone());
            match read_password_line(&mut cursor) {
                Ok(password) => assert!(
                    password.len() <= MAX_PASSWORD_BYTES,
                    "accepted a {}-byte password from {input:02X?}, over the {MAX_PASSWORD_BYTES}-\
                     byte bound this reader promises its caller",
                    password.len()
                ),
                Err(error) => assert!(
                    error.to_string().contains("exceeds"),
                    "the only rejection this reader can produce for in-memory input is the length \
                     bound; got {error} for {input:02X?}"
                ),
            }
        }
    }

    #[test]
    fn any_password_bytes_are_normalized_or_rejected_but_never_panic() {
        let mut rng = crate::codec::property_tests::Rng::new(0x4E4F_524D);
        for _ in 0..2_000 {
            let len = rng.below(24);
            let password = rng.bytes(len);
            if let Ok((username, normalized)) = normalize_provision_credentials("TEST", &password) {
                assert_eq!(username.as_ref(), "TEST");
                let _ =
                    wow_srp::server::SrpVerifier::from_username_and_password(username, normalized);
            }
        }
    }

    #[test]
    fn normalized_credentials_reject_empty_control_and_non_ascii_passwords() {
        for password in [
            b"".as_slice(),
            b"bad\tpass".as_slice(),
            b"p\xC3\xA4ss".as_slice(),
        ] {
            let error = normalize_provision_credentials("TEST", password)
                .unwrap_err()
                .to_string();
            assert_eq!(
                error,
                "invalid password: expected 1-16 non-control ASCII bytes"
            );
        }
    }
}
