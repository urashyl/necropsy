// SPDX-License-Identifier: AGPL-3.0-or-later
//! Outgoing email.
//!
//! SMTP settings come only from environment variables (never from files in
//! the repository):
//!
//! | Variable | Default | |
//! | --- | --- | --- |
//! | `NECROPSY_SMTP_HOST` | unset | Enables SMTP when set. |
//! | `NECROPSY_SMTP_PORT` | 587 (465 for `tls`, 25 for `none`) | |
//! | `NECROPSY_SMTP_USERNAME` / `NECROPSY_SMTP_PASSWORD` | unset | Both or neither. |
//! | `NECROPSY_SMTP_FROM` | required with a host | e.g. `necropsy <no-reply@example.com>` |
//! | `NECROPSY_SMTP_TLS` | `starttls` | `starttls`, `tls` or `none`. |
//!
//! With no SMTP host, mail is disabled and the site closes registration and
//! password reset. For local development only, `NECROPSY_DEV_MAIL_LOG=1`
//! prints each message to the server log instead, which includes the
//! recipient and any links, so it must never be set in production.

use std::sync::{Arc, Mutex};

use lettre::message::Mailbox;
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::AsyncSmtpTransport;
use lettre::{AsyncTransport, Message, Tokio1Executor};

#[derive(Clone, Debug)]
pub struct Mail {
    pub to: String,
    pub subject: String,
    pub body: String,
}

#[derive(Clone)]
pub enum Mailer {
    Disabled,
    Smtp {
        transport: Box<AsyncSmtpTransport<Tokio1Executor>>,
        from: Box<Mailbox>,
        /// Host shown on the admin page. Never a secret.
        host: String,
    },
    /// Development only: print messages to the log.
    Log,
    /// Tests: keep messages in memory.
    Memory(Arc<Mutex<Vec<Mail>>>),
}

impl Mailer {
    /// Builds the mailer from the environment. Misconfiguration is an error
    /// rather than a silent fallback, so a typo cannot quietly disable mail.
    pub fn from_env() -> Result<Self, String> {
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());

        let Some(host) = get("NECROPSY_SMTP_HOST") else {
            return Ok(if get("NECROPSY_DEV_MAIL_LOG").as_deref() == Some("1") {
                Mailer::Log
            } else {
                Mailer::Disabled
            });
        };

        let tls = get("NECROPSY_SMTP_TLS").unwrap_or_else(|| "starttls".into());
        let default_port = match tls.as_str() {
            "tls" => 465,
            "none" => 25,
            _ => 587,
        };
        let port = match get("NECROPSY_SMTP_PORT") {
            Some(p) => p
                .parse()
                .map_err(|_| "NECROPSY_SMTP_PORT is not a port number")?,
            None => default_port,
        };
        let from: Mailbox = get("NECROPSY_SMTP_FROM")
            .ok_or("NECROPSY_SMTP_FROM is required when NECROPSY_SMTP_HOST is set")?
            .parse()
            .map_err(|_| "NECROPSY_SMTP_FROM is not a valid mailbox")?;

        let builder = match tls.as_str() {
            "starttls" => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&host),
            "tls" => AsyncSmtpTransport::<Tokio1Executor>::relay(&host),
            "none" => Ok(AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(
                &host,
            )),
            other => {
                return Err(format!(
                    "NECROPSY_SMTP_TLS must be starttls, tls or none, not {other}"
                ))
            }
        }
        .map_err(|e| format!("invalid SMTP host: {e}"))?
        .port(port);

        let builder = match (get("NECROPSY_SMTP_USERNAME"), get("NECROPSY_SMTP_PASSWORD")) {
            (Some(u), Some(p)) => builder.credentials(Credentials::new(u, p)),
            (None, None) => builder,
            _ => {
                return Err(
                    "set both NECROPSY_SMTP_USERNAME and NECROPSY_SMTP_PASSWORD, or neither".into(),
                )
            }
        };

        Ok(Mailer::Smtp {
            transport: Box::new(builder.build()),
            from: Box::new(from),
            host,
        })
    }

    /// Whether messages can be sent. Registration and password reset are
    /// only offered when this is true.
    pub fn enabled(&self) -> bool {
        !matches!(self, Mailer::Disabled)
    }

    /// One-line description for the admin page.
    pub fn describe(&self) -> String {
        match self {
            Mailer::Disabled => "Not configured".into(),
            Mailer::Smtp { host, .. } => format!("SMTP via {host}"),
            Mailer::Log => "Development log (not sent)".into(),
            Mailer::Memory(_) => "In memory".into(),
        }
    }

    pub async fn send(&self, mail: Mail) -> Result<(), String> {
        match self {
            Mailer::Disabled => Err("email is not configured".into()),
            Mailer::Log => {
                tracing::info!("mail to {}: {}\n{}", mail.to, mail.subject, mail.body);
                Ok(())
            }
            Mailer::Memory(store) => {
                store.lock().unwrap_or_else(|e| e.into_inner()).push(mail);
                Ok(())
            }
            Mailer::Smtp {
                transport, from, ..
            } => {
                let to: Mailbox = mail
                    .to
                    .parse()
                    .map_err(|_| "invalid recipient address".to_owned())?;
                let msg = Message::builder()
                    .from((**from).clone())
                    .to(to)
                    .subject(mail.subject)
                    .body(mail.body)
                    .map_err(|e| e.to_string())?;
                transport
                    .send(msg)
                    .await
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            }
        }
    }
}
