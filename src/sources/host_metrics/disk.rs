use std::path::Path;

use vector_lib::{configurable::configurable_component, metric_tags};

use super::{FilterList, HostMetrics, default_all_devices, example_devices};

/// Options for the disk metrics collector.
#[configurable_component]
#[derive(Clone, Debug, Default)]
pub struct DiskConfig {
    /// Lists of device name patterns to include or exclude in gathering
    /// I/O utilization metrics.
    #[configurable(metadata(docs::examples = "example_devices()"))]
    #[serde(default = "default_all_devices")]
    devices: FilterList,
}

impl HostMetrics {
    pub async fn disk_metrics(&self, output: &mut super::MetricsBuffer) {
        output.name = "disk";
        #[cfg(target_os = "linux")]
        self.disk_metrics_linux(output);
        #[cfg(not(target_os = "linux"))]
        self.disk_metrics_sysinfo(output);
    }

    /// Read /proc/diskstats via procfs for all four metrics with the same
    /// device names and semantics as heim. Respects PROCFS_ROOT for
    /// containerised deployments.
    #[cfg(target_os = "linux")]
    fn disk_metrics_linux(&self, output: &mut super::MetricsBuffer) {
        use crate::internal_events::HostMetricsScrapeDetailError;
        use procfs::{DiskStats, FromRead};

        const SECTOR_SIZE: f64 = 512.0;

        let procfs_root =
            std::env::var("PROCFS_ROOT").unwrap_or_else(|_| "/proc".to_string());
        let path = format!("{procfs_root}/diskstats");

        let stats = match DiskStats::from_file(&path) {
            Ok(s) => s,
            Err(error) => {
                emit!(HostMetricsScrapeDetailError {
                    message: "Failed to read disk I/O stats.",
                    error,
                });
                return;
            }
        };

        for stat in &stats.0 {
            if !self
                .config
                .disk
                .devices
                .contains_path(Some(Path::new(&stat.name)))
            {
                continue;
            }

            let tags = metric_tags! {
                "device" => stat.name.clone()
            };
            output.counter(
                "disk_read_bytes_total",
                stat.sectors_read as f64 * SECTOR_SIZE,
                tags.clone(),
            );
            output.counter(
                "disk_reads_completed_total",
                stat.reads as f64,
                tags.clone(),
            );
            output.counter(
                "disk_written_bytes_total",
                stat.sectors_written as f64 * SECTOR_SIZE,
                tags.clone(),
            );
            output.counter("disk_writes_completed_total", stat.writes as f64, tags);
        }
    }

    /// Use sysinfo for disk I/O bytes on non-linux platforms. Read/write
    /// counts are not available through sysinfo, only bytes.
    #[cfg(not(target_os = "linux"))]
    fn disk_metrics_sysinfo(&self, output: &mut super::MetricsBuffer) {
        use sysinfo::Disks;

        let disks = Disks::new_with_refreshed_list();
        for disk in disks.list() {
            let name = disk.name().to_string_lossy();
            if !self
                .config
                .disk
                .devices
                .contains_str(Some(name.as_ref()))
            {
                continue;
            }

            let usage = disk.usage();
            let tags = metric_tags! {
                "device" => name.into_owned()
            };
            output.counter(
                "disk_read_bytes_total",
                usage.total_read_bytes as f64,
                tags.clone(),
            );
            output.counter(
                "disk_written_bytes_total",
                usage.total_written_bytes as f64,
                tags,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        super::{
            HostMetrics, HostMetricsConfig, MetricsBuffer,
            tests::{all_counters, assert_filtered_metrics, count_name, count_tag},
        },
        DiskConfig,
    };

    #[tokio::test]
    async fn generates_disk_metrics() {
        let mut buffer = MetricsBuffer::new(None);
        HostMetrics::new(HostMetricsConfig::default())
            .disk_metrics(&mut buffer)
            .await;
        let metrics = buffer.metrics;

        // linux emits all four metrics per device from /proc/diskstats
        #[cfg(target_os = "linux")]
        {
            assert!(!metrics.is_empty());
            assert!(metrics.len() % 4 == 0);
            assert!(all_counters(&metrics));

            for name in &[
                "disk_read_bytes_total",
                "disk_reads_completed_total",
                "disk_written_bytes_total",
                "disk_writes_completed_total",
            ] {
                assert_eq!(count_name(&metrics, name), metrics.len() / 4, "name={name}");
            }

            assert_eq!(count_tag(&metrics, "device"), metrics.len());
        }

        // non-linux emits two metrics per device (bytes only) via sysinfo
        #[cfg(not(target_os = "linux"))]
        {
            // sysinfo handles RAID errors gracefully, so this should not crash
            assert!(all_counters(&metrics));
            if !metrics.is_empty() {
                assert!(metrics.len() % 2 == 0);
                assert_eq!(count_tag(&metrics, "device"), metrics.len());
            }
        }
    }

    #[tokio::test]
    async fn filters_disk_metrics_on_device() {
        assert_filtered_metrics("device", |devices| async move {
            let mut buffer = MetricsBuffer::new(None);
            HostMetrics::new(HostMetricsConfig {
                disk: DiskConfig { devices },
                ..Default::default()
            })
            .disk_metrics(&mut buffer)
            .await;
            buffer.metrics
        })
        .await;
    }
}
