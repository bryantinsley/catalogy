use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "catalogy",
    version,
    about = "Local-first semantic media search engine"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Scan directories for media files
    Scan {
        /// Directory path to scan
        #[arg(long)]
        path: Option<String>,

        /// Watch for filesystem changes
        #[arg(long)]
        watch: bool,
    },

    /// Process job queue (extract, embed, index)
    Ingest {
        /// Number of worker threads
        #[arg(long)]
        workers: Option<u32>,

        /// Only process specific stages
        #[arg(long)]
        stages: Option<String>,
    },

    /// Search the media catalog
    Search {
        /// Search query text
        query: String,

        /// Maximum number of results
        #[arg(long, default_value = "20")]
        limit: usize,

        /// Filter by media type (image, video)
        #[arg(long, name = "type")]
        media_type: Option<String>,

        /// Filter by date (after)
        #[arg(long)]
        after: Option<String>,
    },

    /// Show queue and catalog statistics
    Status,

    /// Manage embedding models and re-embed catalog
    Reembed {
        /// Register a new model
        #[arg(long)]
        register: bool,

        /// Activate a model for re-embedding
        #[arg(long)]
        activate: bool,

        /// Model ID
        #[arg(long)]
        model_id: Option<String>,

        /// Path to ONNX model file
        #[arg(long)]
        model_path: Option<String>,

        /// Rebuild ANN index after re-embedding
        #[arg(long)]
        rebuild_index: bool,
    },

    /// Start HTTP API server
    Serve {
        /// Port to listen on
        #[arg(long, default_value = "8080")]
        port: u16,
    },

    /// Show or edit configuration
    Config,
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Scan { .. } => {
            println!("scan: not yet implemented");
        }
        Commands::Ingest { .. } => {
            println!("ingest: not yet implemented");
        }
        Commands::Search { .. } => {
            println!("search: not yet implemented");
        }
        Commands::Status => {
            println!("status: not yet implemented");
        }
        Commands::Reembed { .. } => {
            println!("reembed: not yet implemented");
        }
        Commands::Serve { .. } => {
            println!("serve: not yet implemented");
        }
        Commands::Config => {
            println!("config: not yet implemented");
        }
    }
}
