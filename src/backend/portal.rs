//! Wayland, through the desktop's RemoteDesktop portal (GNOME, KDE Plasma).
//!
//! The portal takes key symbols rather than key codes, so the compositor picks the
//! right key and modifiers for whatever layout is active.

use std::path::PathBuf;
use std::time::Duration;

use ashpd::desktop::remote_desktop::{DeviceType, KeyState, RemoteDesktop, SelectDevicesOptions};
use ashpd::desktop::{PersistMode, ResponseError, Session};
use xkeysym::Keysym;

use super::Backend;

pub struct Portal {
    runtime: Option<tokio::runtime::Runtime>,
    live: Option<(RemoteDesktop, Session<RemoteDesktop>)>,
}

impl Portal {
    pub fn new() -> Self {
        Self {
            runtime: None,
            live: None,
        }
    }
}

/// Where the portal's "don't ask again" token is kept between runs.
fn token_path() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".config")))?;
    Some(config.join("sendkeys").join("portal-token"))
}

fn explain(error: ashpd::Error) -> String {
    match error {
        ashpd::Error::Response(ResponseError::Cancelled) => {
            "Permission to send keystrokes was declined.".into()
        }
        ashpd::Error::PortalNotFound(_) => {
            "This desktop has no RemoteDesktop portal, which is needed to type on Wayland \
             (GNOME and KDE Plasma provide it)."
                .into()
        }
        ashpd::Error::Response(ResponseError::Other) => {
            "The desktop refused to let this app send keystrokes.".into()
        }
        other => format!("Desktop portal error: {other}"),
    }
}

async fn open() -> ashpd::Result<(RemoteDesktop, Session<RemoteDesktop>)> {
    let proxy = RemoteDesktop::new().await?;
    let session = proxy.create_session(Default::default()).await?;

    let keyboard = ashpd::enumflags2::BitFlags::from(DeviceType::Keyboard);
    let mut options = SelectDevicesOptions::default().set_devices(keyboard);
    let saved = token_path().and_then(|p| std::fs::read_to_string(p).ok());
    // Remembering the permission needs version 2 of the portal.
    if proxy.version() >= 2 {
        options = options
            .set_persist_mode(PersistMode::ExplicitlyRevoked)
            .set_restore_token(saved.as_deref().map(str::trim));
    }
    proxy.select_devices(&session, options).await?.response()?;

    let granted = proxy
        .start(&session, None, Default::default())
        .await?
        .response()?;
    if !granted.devices().contains(DeviceType::Keyboard) {
        return Err(ashpd::Error::Response(ResponseError::Cancelled));
    }
    if let (Some(token), Some(path)) = (granted.restore_token(), token_path()) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, token);
    }
    Ok((proxy, session))
}

impl Backend for Portal {
    fn name(&self) -> &'static str {
        "Wayland (desktop portal)"
    }

    fn begin(&mut self) -> Result<(), String> {
        if self.live.is_some() {
            return Ok(());
        }
        if self.runtime.is_none() {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .map_err(|e| format!("Cannot start the portal client: {e}"))?;
            self.runtime = Some(runtime);
        }
        let runtime = self.runtime.as_ref().expect("runtime was just created");
        // The session is kept for the life of the app, so the desktop asks at most once.
        self.live = Some(runtime.block_on(open()).map_err(explain)?);
        Ok(())
    }

    fn type_char(&mut self, c: char, hold: Duration) -> Result<(), String> {
        let (Some(runtime), Some((proxy, session))) = (&self.runtime, &self.live) else {
            return Err("not connected".into());
        };
        let keysym = match c {
            '\n' => Keysym::Return,
            '\t' => Keysym::Tab,
            _ => Keysym::from_char(c),
        };
        if keysym == Keysym::NoSymbol {
            return Err(format!("There is no key for {c:?}."));
        }
        let keysym = keysym.raw() as i32;
        let send = |state| {
            runtime.block_on(proxy.notify_keyboard_keysym(session, keysym, state, Default::default()))
        };
        let result = send(KeyState::Pressed).and_then(|()| {
            std::thread::sleep(hold);
            send(KeyState::Released)
        });
        if let Err(error) = result {
            // The session is gone (for example revoked from the desktop's indicator);
            // ask again on the next run.
            self.live = None;
            return Err(explain(error));
        }
        Ok(())
    }

    fn end(&mut self) {}
}

impl Drop for Portal {
    fn drop(&mut self) {
        if let (Some(runtime), Some((_, session))) = (&self.runtime, &self.live) {
            let _ = runtime.block_on(session.close());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Talks to the real portal but stops before the step that asks for permission.
    #[test]
    #[ignore = "needs a desktop session with the RemoteDesktop portal"]
    fn can_open_and_close_a_portal_session() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let proxy = RemoteDesktop::new().await.unwrap();
            assert!(proxy.version() >= 1);
            let devices = proxy.available_device_types().await.unwrap();
            assert!(devices.contains(DeviceType::Keyboard));
            let session = proxy.create_session(Default::default()).await.unwrap();
            session.close().await.unwrap();
        });
    }
}
