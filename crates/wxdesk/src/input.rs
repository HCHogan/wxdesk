//! The mouse, the keyboard and the clipboard, through xdotool and xclip
//! (XTEST and the X selection), in the X display the driver runs in.

use anyhow::{Context, Result, bail};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

async fn xdotool(args: &[&str]) -> Result<()> {
    let status = Command::new("xdotool")
        .args(args)
        .stdin(Stdio::null())
        .status()
        .await
        .context("xdotool")?;
    if !status.success() {
        bail!("xdotool {args:?}: {status}");
    }
    Ok(())
}

pub async fn click(x: i32, y: i32) -> Result<()> {
    xdotool(&["mousemove", &x.to_string(), &y.to_string(), "click", "1"]).await
}

pub async fn right_click(x: i32, y: i32) -> Result<()> {
    xdotool(&["mousemove", &x.to_string(), &y.to_string(), "click", "3"]).await
}

/// Moves the mouse out of the way, so no hover covers anything.
pub async fn park() -> Result<()> {
    xdotool(&["mousemove", "0", "0"]).await
}

/// Moves the windows called `name` (a regex) to the top left and makes
/// them as large as the display.
pub async fn fill_screen(name: &str) -> Result<()> {
    xdotool(&[
        "search",
        "--onlyvisible",
        "--name",
        name,
        "windowmove",
        "%@",
        "0",
        "0",
        "windowsize",
        "%@",
        "100%",
        "100%",
    ])
    .await
}

pub async fn key(k: &str) -> Result<()> {
    xdotool(&["key", k]).await
}

/// Puts `data` on the clipboard as `mime`. xclip stays behind, holding the
/// selection, until something else takes it.
pub async fn set_clipboard(mime: &str, data: &[u8]) -> Result<()> {
    let mut child = Command::new("xclip")
        .args(["-selection", "clipboard", "-t", mime, "-i"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("xclip")?;
    let mut stdin = child.stdin.take().expect("piped");
    stdin.write_all(data).await?;
    drop(stdin);
    let status = child.wait().await?;
    if !status.success() {
        bail!("xclip -i: {status}");
    }
    Ok(())
}

/// What the clipboard holds as `mime`.
pub async fn clipboard(mime: &str) -> Result<Vec<u8>> {
    let out = tokio::time::timeout(
        Duration::from_secs(5),
        Command::new("xclip")
            .args(["-selection", "clipboard", "-t", mime, "-o"])
            .stdin(Stdio::null())
            .output(),
    )
    .await
    .context("xclip -o timed out")??;
    if !out.status.success() || out.stdout.is_empty() {
        bail!("the clipboard has no {mime}");
    }
    Ok(out.stdout)
}

/// Pastes `text` where the keyboard is.
pub async fn paste_text(text: &str) -> Result<()> {
    set_clipboard("UTF8_STRING", text.as_bytes()).await?;
    pause(150).await;
    key("ctrl+v").await
}

/// Pastes the file at `path` as a file, the way a file manager copies one.
pub async fn paste_file(path: &Path) -> Result<()> {
    let uri = format!("file://{}\r\n", path.display());
    set_clipboard("text/uri-list", uri.as_bytes()).await?;
    pause(150).await;
    key("ctrl+v").await
}

pub async fn pause(ms: u64) {
    tokio::time::sleep(Duration::from_millis(ms)).await;
}
