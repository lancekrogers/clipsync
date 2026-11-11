//! `clipsync history delete|clear`, sent through the daemon's control socket.
use crate::config::Config;
use crate::control::{self, Command, HistoryRemovalView};
use anyhow::{bail, Context, Result};
use std::io::{BufRead, IsTerminal, Write};
use uuid::Uuid;

pub async fn delete(config: &Config, id: Uuid) -> Result<()> {
    require_feature(config, control::FEATURE_HISTORY_DELETE).await?;
    let removal = send(config, Command::HistoryDelete { id }).await?;
    if removal.removed == 0 {
        println!("No history entry {id} on this device");
    } else {
        println!("Deleted history entry {id}");
    }
    print_summary(&removal);
    Ok(())
}

pub async fn clear(config: &Config, yes: bool) -> Result<()> {
    require_feature(config, control::FEATURE_HISTORY_CLEAR).await?;
    if !yes && !confirm_clear()? {
        println!("History not cleared");
        return Ok(());
    }
    let removal = send(config, Command::HistoryClear).await?;
    println!("Cleared {} history entries", removal.removed);
    print_summary(&removal);
    Ok(())
}

async fn send(config: &Config, command: Command) -> Result<HistoryRemovalView> {
    let response = control::request(
        &control::socket_path(config),
        command,
        control::max_frame_bytes(config),
    )
    .await?;
    response
        .history_removal
        .context("Daemon did not report a history result")
}

// Older daemons cannot parse these commands and drop the connection unanswered.
async fn require_feature(config: &Config, feature: &str) -> Result<()> {
    let status = control::request(
        &control::socket_path(config),
        Command::Status,
        control::max_frame_bytes(config),
    )
    .await?;
    let supported = status
        .features
        .is_some_and(|features| features.iter().any(|f| f == feature));
    if !supported {
        bail!("The running ClipSync daemon does not support {feature}; restart it after upgrading");
    }
    Ok(())
}

fn confirm_clear() -> Result<bool> {
    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        bail!("Refusing to clear history without confirmation; pass --yes");
    }
    print!(
        "Clear clipboard history and the clipboard on this device and all connected devices? [y/N] "
    );
    std::io::stdout().flush()?;
    let mut answer = String::new();
    stdin.lock().read_line(&mut answer)?;
    Ok(confirmed(&answer))
}

fn confirmed(answer: &str) -> bool {
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

fn print_summary(removal: &HistoryRemovalView) {
    if removal.clipboard_cleared {
        println!("Cleared the clipboard on this device");
    }
    println!("Sent to {} connected device(s)", removal.peers_notified);
    if removal.peers_skipped > 0 {
        println!(
            "Not sent to {} connected device(s) running an older ClipSync or busy; remove it there directly",
            removal.peers_skipped
        );
    }
    println!("Devices that are offline now keep their copy");
}

#[cfg(test)]
mod tests {
    use super::confirmed;

    #[test]
    fn only_an_explicit_yes_confirms() {
        assert!(confirmed("y\n"));
        assert!(confirmed(" YES "));
        assert!(!confirmed("\n"));
        assert!(!confirmed("n"));
        assert!(!confirmed("yep"));
    }
}
