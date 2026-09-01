use std::path::PathBuf;

use anyhow::Result;
use reagent_core::{build_provider, AgentRunConfig, Runtime};

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    let cli = Cli::parse(std::env::args().skip(1).collect());
    let repo_root = std::env::current_dir()?;
    let artifact_dir = cli
        .artifact_dir
        .unwrap_or_else(|| repo_root.join("artifacts"));
    let capabilities_dir = cli
        .capabilities_dir
        .unwrap_or_else(|| repo_root.join("capabilities"));

    let runtime = Runtime::load(capabilities_dir)?;
    let provider = build_provider(&cli.provider)?;

    let config = AgentRunConfig {
        user_prompt: cli.prompt,
        artifact_dir,
        max_steps: 24,
        input_files: Vec::new(),
        event_tx: None,
    };

    let output = runtime.run(provider, config).await?;

    println!("{}", output.final_message);
    println!("run_id: {}", output.run_id);
    for artifact in output.artifacts {
        println!("artifact: {}", artifact);
    }
    if let Some(path) = output.event_log_path {
        println!("trajectory: {}", path.display());
    }
    Ok(())
}

#[derive(Debug)]
struct Cli {
    prompt: String,
    artifact_dir: Option<PathBuf>,
    capabilities_dir: Option<PathBuf>,
    provider: String,
}

impl Cli {
    fn parse(args: Vec<String>) -> Self {
        let mut prompt = None;
        let mut artifact_dir = None;
        let mut capabilities_dir = None;
        let mut provider = "deepseek".to_string();

        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--out" => {
                    i += 1;
                    artifact_dir = Some(PathBuf::from(
                        args.get(i).expect("--out requires a directory").as_str(),
                    ));
                }
                "--capabilities" => {
                    i += 1;
                    capabilities_dir = Some(PathBuf::from(
                        args.get(i)
                            .expect("--capabilities requires a directory")
                            .as_str(),
                    ));
                }
                "--provider" => {
                    i += 1;
                    provider = args
                        .get(i)
                        .expect("--provider requires a value")
                        .to_string();
                }
                value => {
                    prompt = Some(match prompt {
                        Some(existing) => format!("{existing} {value}"),
                        None => value.to_string(),
                    });
                }
            }
            i += 1;
        }

        Self {
            prompt: prompt.unwrap_or_else(|| {
                "我要做一个关于飓法work的易拉宝，要求是Pdf给我，内容你自己编".to_string()
            }),
            artifact_dir,
            capabilities_dir,
            provider,
        }
    }
}
