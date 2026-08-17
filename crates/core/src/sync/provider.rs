use std::path::Path;

/// Provider-neutral object boundary. OneDrive/plugin adapters implement only
/// immutable object discovery/read; verification and durable registration stay
/// inside Core.
pub trait SegmentProvider {
    fn pending_segments(&self) -> anyhow::Result<Vec<Vec<u8>>>;
}

pub fn intake_provider_segments(
    provider: &dyn SegmentProvider,
    spool_dir: &Path,
) -> anyhow::Result<usize> {
    let mut accepted = 0;
    for bytes in provider.pending_segments()? {
        super::spool::intake_segment(spool_dir, &bytes, None, |_| Ok(()))?;
        accepted += 1;
    }
    Ok(accepted)
}
