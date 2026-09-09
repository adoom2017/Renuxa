use std::path::PathBuf;

use clap::{Parser, Subcommand};
use im_channel_gateway::agent::build_agent_backend;
use im_channel_gateway::config::{load_config, write_default_config};
use im_channel_gateway::manager::MultiChannelManager;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "im-channel-gateway")]
#[command(about = "Standalone IM gateway (Telegram, WeChat iLink)")]
struct Cli {
    /// Config file path (run, login) or output path for `init` (default: config.toml)
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Write example config.toml (default path: config.toml, or `--config`)
    Init,
    /// Run the gateway (loads config, starts enabled channels)
    Run,
    /// Platform login flows
    Login {
        #[command(subcommand)]
        platform: LoginPlatform,
    },
}

#[derive(Subcommand)]
enum LoginPlatform {
    /// WeChat iLink QR login and register a new bot account
    Wechat,
}

fn config_path(cli: &Cli) -> PathBuf {
    cli.config
        .clone()
        .unwrap_or_else(|| PathBuf::from("config.toml"))
}

#[tokio::main]
async fn main() -> im_channel_gateway::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    match cli.command {
        Commands::Init => {
            let path = config_path(&cli);
            write_default_config(&path)?;
            tracing::info!("wrote {}", path.display());
        }
        Commands::Run => {
            let cfg = load_config(cli.config.as_deref())?;
            let _account_lock = if cfg.channels.wechat.inline_images {
                std::fs::create_dir_all(cfg.data_path())?;
                let file = std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .open(cfg.data_path().join("gateway.lock"))?;
                file.try_lock().map_err(|_| {
                    im_channel_gateway::GatewayError::Other(
                        "another gateway is using this account data directory".into(),
                    )
                })?;
                Some(file)
            } else {
                None
            };
            tracing::info!(
                agent_backend = %cfg.agent.backend,
                default_runner = %cfg.agent.default_runner,
                cli_runners = ?cfg.agent.runners.keys().collect::<Vec<_>>(),
                wechat_enabled = cfg.channels.wechat.base.enabled,
                "im-channel-gateway config loaded"
            );
            let agent = build_agent_backend(&cfg)?;
            let manager = MultiChannelManager::new(cfg.clone(), agent)?;
            if cfg.admin.listen.is_empty() {
                tracing::info!("admin API disabled (empty listen)");
            } else {
                let admin_cfg = cfg.clone();
                let telegram = manager.telegram_channel();
                let wechat = manager.wechat_channel();
                tokio::spawn(async move {
                    if let Err(e) =
                        im_channel_gateway::admin_api::serve(&admin_cfg, telegram, wechat).await
                    {
                        tracing::error!("admin API: {e}");
                    }
                });
            }
            manager.start_all().await?;
            tracing::info!("im-channel-gateway running; Ctrl+C to stop");
            tokio::signal::ctrl_c()
                .await
                .map_err(|e| im_channel_gateway::GatewayError::Other(e.to_string()))?;
            manager.stop_all().await?;
        }
        Commands::Login {
            platform: LoginPlatform::Wechat,
        } => {
            let mut cfg = load_config(cli.config.as_deref())?;
            im_channel_gateway::platforms::wechat::login::run_cli_login(&mut cfg).await?;
        }
    }
    Ok(())
}
