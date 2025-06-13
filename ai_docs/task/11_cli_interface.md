# Task 11: CLI Interface

## Objective
Create the command-line interface for ClipSync with daemon mode, configuration, and control commands.

## Steps

1. **Create src/cli/mod.rs**
   - Command parsing with clap
   - Subcommands structure
   - Output formatting

2. **Define CLI structure**
   ```rust
   #[derive(Parser)]
   #[command(name = "clipsync")]
   #[command(about = "Cross-platform clipboard synchronization")]
   struct Cli {
       #[arg(short, long, value_name = "FILE")]
       config: Option<PathBuf>,
       
       #[arg(short, long, action = ArgAction::Count)]
       verbose: u8,
       
       #[command(subcommand)]
       command: Option<Commands>,
   }
   
   #[derive(Subcommand)]
   enum Commands {
       /// Run in daemon mode
       Daemon {
           #[arg(long)]
           foreground: bool,
       },
       /// Show clipboard history
       History {
           #[arg(short, long, default_value = "10")]
           count: usize,
       },
       /// Control sync status
       Sync {
           #[command(subcommand)]
           action: SyncAction,
       },
       /// Generate configuration
       Init,
   }
   ```

3. **Implement daemon mode**
   - Background process handling
   - PID file management
   - Signal handling (SIGTERM, SIGINT)
   - Logging configuration

4. **Add control commands**
   - Start/stop/restart daemon
   - Pause/resume sync
   - Clear history
   - Show status

5. **Create history interface**
   - List recent entries
   - Search history
   - Copy from history
   - Export history

6. **Add utility commands**
   - Generate default config
   - Test connectivity
   - Show version info
   - Validate configuration

## Success Criteria
- Intuitive command structure
- Daemon runs reliably
- Commands provide useful feedback
- Help text is comprehensive