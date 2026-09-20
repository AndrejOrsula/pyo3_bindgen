//! Resource budgets and platform resource measurements for generation.
//!
//! [`GenerationBudget`] is deliberately independent of [`crate::Config`].  The former bounds
//! the cost of one generation operation; the latter controls the shape of the generated API.
//! Codegen selects and drives this foundation across the generation phases.

use std::{fmt, time::Duration};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct StructuralResources {
    pub(crate) definitions: usize,
    pub(crate) recursion_depth: usize,
    pub(crate) model_nodes: usize,
}

/// Resource categories that can be bounded by a [`GenerationBudget`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceKind {
    /// Number of definitions in the complete generation operation.
    Definitions,
    /// Maximum recursive source/model depth observed by the operation.
    RecursionDepth,
    /// Number of nodes in the normalized model.
    ModelNodes,
    /// Number of bytes in the canonical generated artifact.
    OutputBytes,
    /// Elapsed wall-clock time for the operation.
    WallTime,
    /// Increase in the process peak resident set size during the operation.
    PeakRssDeltaBytes,
}

impl ResourceKind {
    /// Stable machine-readable spelling for this resource category.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Definitions => "definitions",
            Self::RecursionDepth => "recursion_depth",
            Self::ModelNodes => "model_nodes",
            Self::OutputBytes => "output_bytes",
            Self::WallTime => "wall_time",
            Self::PeakRssDeltaBytes => "peak_rss_delta_bytes",
        }
    }
}

impl fmt::Display for ResourceKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Generation phase at which a resource checkpoint is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourcePhase {
    /// Source registration and bounded acquisition bookkeeping.
    Registration,
    /// Schema validation.
    Validation,
    /// Schema-to-model lowering.
    Lowering,
    /// Normalized-model construction.
    Normalization,
    /// Canonical Rust rendering.
    Emission,
}

impl ResourcePhase {
    /// Stable machine-readable spelling for this generation phase.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Registration => "registration",
            Self::Validation => "validation",
            Self::Lowering => "lowering",
            Self::Normalization => "normalization",
            Self::Emission => "emission",
        }
    }
}

impl fmt::Display for ResourcePhase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A typed failure recording an observed value above its configured limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
#[error("resource limit exceeded for {kind} during {phase}: observed {observed}, limit {limit}")]
pub struct ResourceLimitExceeded {
    /// Resource category that exceeded its budget.
    pub kind: ResourceKind,
    /// Generation phase in which the over-limit value was observed.
    pub phase: ResourcePhase,
    /// Observed value. Wall time is represented in nanoseconds.
    pub observed: u128,
    /// Configured limit. Wall time is represented in nanoseconds.
    pub limit: u128,
}

/// A typed failure for a requested measurement that is unavailable on the current target.
///
/// This is distinct from [`ResourceLimitExceeded`]: an unavailable measurement does not prove
/// that the operation exceeded its budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
#[error("resource measurement unavailable for {kind} during {phase}: limit {limit}")]
pub struct ResourceMeasurementUnavailable {
    /// Resource category whose measurement was requested.
    pub kind: ResourceKind,
    /// Generation phase at which the measurement was requested.
    pub phase: ResourcePhase,
    /// Configured finite limit. Wall time is not currently measurement-dependent.
    pub limit: u128,
}

const DEFAULT_MAX_DEFINITIONS: usize = 100_000;
const DEFAULT_MAX_RECURSION_DEPTH: usize = 32;
const DEFAULT_MAX_MODEL_NODES: usize = 1_000_000;
const DEFAULT_MAX_OUTPUT_BYTES: usize = 512 * 1024 * 1024;
const DEFAULT_MAX_WALL_TIME: Duration = Duration::from_secs(300);
const DEFAULT_MAX_PEAK_RSS_DELTA_BYTES: u64 = 2 * 1024 * 1024 * 1024;

const RSS_MEASUREMENT_SUPPORTED: bool = cfg!(any(
    target_os = "android",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "linux",
    target_os = "macos",
    target_os = "netbsd",
    target_os = "openbsd",
    windows,
));

/// Operation-level resource limits for generation.
///
/// The defaults are finite and intentionally leave room above the current checked-in workloads:
/// 100,000 definitions, 32 levels of recursion, one million model nodes, 512 MiB of canonical
/// output, and five minutes of wall time. The recursion default remains below the existing
/// type/parser hard ceiling of 64. Peak RSS is bounded to two GiB where the target adapter
/// is available; it is `None` on targets where this slice cannot measure process RSS.
///
/// This type is not part of [`crate::Config`] because resource policy is selected independently by
/// the programmatic Codegen API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GenerationBudget {
    /// Maximum number of definitions in one generation operation.
    pub max_definitions: usize,
    /// Maximum recursion depth permitted by one generation operation.
    pub max_recursion_depth: usize,
    /// Maximum number of nodes in the normalized model.
    pub max_model_nodes: usize,
    /// Maximum canonical generated artifact size in bytes.
    pub max_output_bytes: usize,
    /// Maximum cooperative wall-clock duration for one operation.
    pub max_wall_time: Duration,
    /// Maximum process peak-RSS increase in bytes, or `None` to leave RSS unbounded.
    pub max_peak_rss_delta_bytes: Option<u64>,
}

impl Default for GenerationBudget {
    fn default() -> Self {
        Self {
            max_definitions: DEFAULT_MAX_DEFINITIONS,
            max_recursion_depth: DEFAULT_MAX_RECURSION_DEPTH,
            max_model_nodes: DEFAULT_MAX_MODEL_NODES,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            max_wall_time: DEFAULT_MAX_WALL_TIME,
            max_peak_rss_delta_bytes: if RSS_MEASUREMENT_SUPPORTED {
                Some(DEFAULT_MAX_PEAK_RSS_DELTA_BYTES)
            } else {
                None
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PeakRssBaseline {
    Disabled,
    Measured(u64),
    Unavailable,
}

/// Tracks one operation's start point and performs cooperative resource checkpoints.
pub(crate) struct BudgetTracker {
    budget: GenerationBudget,
    started_at: std::time::Instant,
    peak_rss_baseline: PeakRssBaseline,
}

impl BudgetTracker {
    /// Start tracking an operation with the selected budget.
    pub(crate) fn new(budget: GenerationBudget) -> Self {
        let peak_rss_baseline = match budget.max_peak_rss_delta_bytes {
            None => PeakRssBaseline::Disabled,
            Some(_) => {
                peak_rss_bytes().map_or(PeakRssBaseline::Unavailable, PeakRssBaseline::Measured)
            }
        };
        Self {
            budget,
            started_at: std::time::Instant::now(),
            peak_rss_baseline,
        }
    }

    /// Check a whole-operation resource at a generation phase boundary.
    pub(crate) fn check_whole_resource(
        &self,
        kind: ResourceKind,
        phase: ResourcePhase,
        observed: u128,
    ) -> crate::PyBindgenResult<()> {
        let Some(limit) = self.limit_for(kind) else {
            return Ok(());
        };
        self.check_value(kind, phase, observed, limit)
    }

    pub(crate) fn check_structure(
        &self,
        phase: ResourcePhase,
        resources: StructuralResources,
    ) -> crate::PyBindgenResult<()> {
        self.check_whole_resource(
            ResourceKind::Definitions,
            phase,
            resources.definitions as u128,
        )?;
        self.check_whole_resource(
            ResourceKind::RecursionDepth,
            phase,
            resources.recursion_depth as u128,
        )?;
        self.check_whole_resource(
            ResourceKind::ModelNodes,
            phase,
            resources.model_nodes as u128,
        )
    }

    /// Check elapsed wall time at a cooperative phase seam.
    pub(crate) fn checkpoint_wall(&self, phase: ResourcePhase) -> crate::PyBindgenResult<()> {
        self.checkpoint_wall_elapsed(phase, self.started_at.elapsed())
    }

    /// Check a process peak-RSS delta at a cooperative phase seam.
    pub(crate) fn checkpoint_peak_rss_delta(
        &self,
        phase: ResourcePhase,
    ) -> crate::PyBindgenResult<()> {
        let Some(limit) = self.budget.max_peak_rss_delta_bytes else {
            return Ok(());
        };
        let baseline = match self.peak_rss_baseline {
            PeakRssBaseline::Disabled => return Ok(()),
            PeakRssBaseline::Measured(baseline) => baseline,
            PeakRssBaseline::Unavailable => {
                return Err(ResourceMeasurementUnavailable {
                    kind: ResourceKind::PeakRssDeltaBytes,
                    phase,
                    limit: u128::from(limit),
                }
                .into());
            }
        };
        let current = peak_rss_bytes().ok_or_else(|| {
            crate::PyBindgenError::from(ResourceMeasurementUnavailable {
                kind: ResourceKind::PeakRssDeltaBytes,
                phase,
                limit: u128::from(limit),
            })
        })?;
        let delta = current.saturating_sub(baseline);
        self.check_value(
            ResourceKind::PeakRssDeltaBytes,
            phase,
            u128::from(delta),
            u128::from(limit),
        )
    }

    fn checkpoint_wall_elapsed(
        &self,
        phase: ResourcePhase,
        elapsed: Duration,
    ) -> crate::PyBindgenResult<()> {
        self.check_value(
            ResourceKind::WallTime,
            phase,
            elapsed.as_nanos(),
            self.budget.max_wall_time.as_nanos(),
        )
    }

    fn limit_for(&self, kind: ResourceKind) -> Option<u128> {
        match kind {
            ResourceKind::Definitions => Some(self.budget.max_definitions as u128),
            ResourceKind::RecursionDepth => Some(self.budget.max_recursion_depth as u128),
            ResourceKind::ModelNodes => Some(self.budget.max_model_nodes as u128),
            ResourceKind::OutputBytes => Some(self.budget.max_output_bytes as u128),
            ResourceKind::WallTime => Some(self.budget.max_wall_time.as_nanos()),
            ResourceKind::PeakRssDeltaBytes => self.budget.max_peak_rss_delta_bytes.map(u128::from),
        }
    }

    fn check_value(
        &self,
        kind: ResourceKind,
        phase: ResourcePhase,
        observed: u128,
        limit: u128,
    ) -> crate::PyBindgenResult<()> {
        if observed > limit {
            return Err(ResourceLimitExceeded {
                kind,
                phase,
                observed,
                limit,
            }
            .into());
        }
        Ok(())
    }
}

#[cfg(any(
    target_os = "android",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "linux",
    target_os = "netbsd",
    target_os = "openbsd",
))]
const RSS_UNIT_BYTES: u64 = 1024;

#[cfg(target_os = "macos")]
const RSS_UNIT_BYTES: u64 = 1;

#[cfg(any(
    target_os = "android",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "linux",
    target_os = "macos",
    target_os = "netbsd",
    target_os = "openbsd",
))]
fn rss_bytes_from_raw(raw: libc::c_long) -> Option<u64> {
    u64::try_from(raw).ok()?.checked_mul(RSS_UNIT_BYTES)
}

#[cfg(any(
    target_os = "android",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "linux",
    target_os = "macos",
    target_os = "netbsd",
    target_os = "openbsd",
))]
fn peak_rss_bytes() -> Option<u64> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    let status = unsafe {
        // SAFETY: `usage.as_mut_ptr()` is aligned and valid for one
        // `libc::rusage` result, and the storage remains live for the call.
        // `RUSAGE_SELF` is a valid selector; the return status is checked
        // before the possibly uninitialized value is assumed initialized.
        libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr())
    };
    if status != 0 {
        return None;
    }
    let usage = unsafe {
        // SAFETY: the preceding `getrusage` call returned zero, so its output
        // pointer was populated with a complete `rusage` value before it is
        // read here.
        usage.assume_init()
    };
    rss_bytes_from_raw(usage.ru_maxrss)
}

#[cfg(windows)]
fn peak_rss_bytes() -> Option<u64> {
    use windows_sys::Win32::System::{
        ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
        Threading::GetCurrentProcess,
    };

    let mut counters = PROCESS_MEMORY_COUNTERS {
        cb: u32::try_from(std::mem::size_of::<PROCESS_MEMORY_COUNTERS>()).ok()?,
        ..PROCESS_MEMORY_COUNTERS::default()
    };
    let status = unsafe {
        // SAFETY: `GetCurrentProcess` supplies the current process's valid
        // pseudo-handle and does not transfer ownership. `counters` is an
        // aligned writable `PROCESS_MEMORY_COUNTERS` buffer, and `cb` is its
        // exact byte size. The BOOL result is checked before reading the
        // returned peak-working-set field.
        K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb)
    };
    (status != 0).then(|| u64::try_from(counters.PeakWorkingSetSize).ok())?
}

#[cfg(not(any(
    target_os = "android",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "linux",
    target_os = "macos",
    target_os = "netbsd",
    target_os = "openbsd",
    windows,
)))]
fn peak_rss_bytes() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_finite_and_leave_headroom() {
        let budget = GenerationBudget::default();
        assert!(budget.max_definitions > 10_000);
        assert!(budget.max_recursion_depth < 64);
        assert!(budget.max_model_nodes > 100_000);
        assert!(budget.max_output_bytes > 6_500_460);
        assert!(budget.max_wall_time > Duration::from_secs(1));
        if RSS_MEASUREMENT_SUPPORTED {
            assert_eq!(
                budget.max_peak_rss_delta_bytes,
                Some(DEFAULT_MAX_PEAK_RSS_DELTA_BYTES)
            );
        } else {
            assert_eq!(budget.max_peak_rss_delta_bytes, None);
        }
    }

    #[test]
    fn whole_resource_failure_preserves_exact_typed_fields() {
        let tracker = BudgetTracker::new(GenerationBudget {
            max_output_bytes: 32,
            ..GenerationBudget::default()
        });
        let error = tracker
            .check_whole_resource(ResourceKind::OutputBytes, ResourcePhase::Emission, 33)
            .expect_err("the output budget must fail");
        let crate::PyBindgenError::ResourceLimitExceeded(error) = error else {
            panic!("expected a typed resource limit error");
        };
        assert_eq!(
            error,
            ResourceLimitExceeded {
                kind: ResourceKind::OutputBytes,
                phase: ResourcePhase::Emission,
                observed: 33,
                limit: 32,
            }
        );
    }

    #[test]
    fn wall_checkpoint_is_deterministic_at_the_boundary() {
        let tracker = BudgetTracker::new(GenerationBudget {
            max_wall_time: Duration::from_secs(5),
            ..GenerationBudget::default()
        });
        assert!(tracker
            .checkpoint_wall_elapsed(ResourcePhase::Validation, Duration::from_secs(5))
            .is_ok());
        let error = tracker
            .checkpoint_wall_elapsed(
                ResourcePhase::Validation,
                Duration::from_secs(5) + Duration::from_nanos(1),
            )
            .expect_err("one nanosecond over the wall budget must fail");
        let crate::PyBindgenError::ResourceLimitExceeded(error) = error else {
            panic!("expected a typed wall-time limit error");
        };
        assert_eq!(error.kind, ResourceKind::WallTime);
        assert_eq!(error.phase, ResourcePhase::Validation);
        assert_eq!(error.observed, 5_000_000_001);
        assert_eq!(error.limit, 5_000_000_000);
    }

    #[cfg(any(
        target_os = "android",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "linux",
        target_os = "macos",
        target_os = "netbsd",
        target_os = "openbsd",
    ))]
    #[test]
    fn unix_peak_rss_adapter_uses_byte_units() {
        let expected = if cfg!(target_os = "macos") {
            2
        } else {
            2 * 1024
        };
        assert_eq!(rss_bytes_from_raw(2), Some(expected));
        assert!(peak_rss_bytes().is_some());
    }

    #[cfg(not(any(
        target_os = "android",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "linux",
        target_os = "macos",
        target_os = "netbsd",
        target_os = "openbsd",
        windows,
    )))]
    #[test]
    fn unsupported_peak_rss_is_a_typed_failure() {
        assert_eq!(peak_rss_bytes(), None);
        let tracker = BudgetTracker::new(GenerationBudget {
            max_peak_rss_delta_bytes: Some(1),
            ..GenerationBudget::default()
        });
        let error = tracker
            .checkpoint_peak_rss_delta(ResourcePhase::Registration)
            .expect_err("finite RSS on an unsupported target must fail typed");
        let crate::PyBindgenError::ResourceMeasurementUnavailable(error) = error else {
            panic!("expected a typed measurement-unavailable error");
        };
        assert_eq!(error.kind, ResourceKind::PeakRssDeltaBytes);
        assert_eq!(error.phase, ResourcePhase::Registration);
        assert_eq!(error.limit, 1);
    }
}
