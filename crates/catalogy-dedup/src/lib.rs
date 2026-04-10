pub mod cross_video;
pub mod exact;
pub mod report;
pub mod visual;

pub use cross_video::{find_cross_video_duplicates, CrossVideoDuplicate};
pub use exact::{find_exact_duplicates, DuplicateFile, DuplicateSet};
pub use report::{format_cross_video_report, format_exact_report, format_visual_report};
pub use visual::{find_visual_duplicates, VisualDuplicateCluster, VisualDuplicateItem};
