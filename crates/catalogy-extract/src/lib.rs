mod extract;
mod thumbnail;
mod worker;

pub use extract::{
    build_ffmpeg_args, extract_frames, parse_frame_files, process_frames_in_batches,
    ExtractionStrategy, FrameOutput,
};
pub use thumbnail::generate_thumbnail;
pub use worker::{run_extract_frames_worker, ExtractFramesResult};
