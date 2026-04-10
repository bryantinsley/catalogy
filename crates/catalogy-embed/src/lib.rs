pub mod image_encoder;
pub mod session;
pub mod text_encoder;
pub mod worker;

pub use session::{cosine_similarity, dedup_frames, l2_normalize, mean_pool, EmbedSession};
pub use worker::{aggregate_video_frames, run_embed_worker, run_reembed_worker};
