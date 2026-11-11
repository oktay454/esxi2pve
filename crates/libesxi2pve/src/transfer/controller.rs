//! Dengeli denetim; gecikme penceresi, yavaş artırma ve hızlı azaltma.
use std::{
	collections::VecDeque,
	fs,
	os::unix::fs::MetadataExt,
	path::{Path, PathBuf},
};
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Pressure {
	pub queue: Option<u64>,
	pub stall: Option<f64>,
}
pub(super) struct Probe {
	path: PathBuf,
	previous: Option<u64>,
}
impl Probe {
	pub fn new(target: &Path) -> Self {
		let device = fs::metadata(target)
			.map(|m| {
				use std::os::unix::fs::FileTypeExt;
				if m.file_type().is_block_device() {
					m.rdev()
				} else {
					m.dev()
				}
			})
			.unwrap_or(0);
		let major = (device >> 8 & 0xfff) | (device >> 32 & 0xfffff000);
		let minor = (device & 0xff) | (device >> 12 & 0xffffff00);
		Self {
			path: format!("/sys/dev/block/{major}:{minor}/stat").into(),
			previous: None,
		}
	}
	pub fn sample(&mut self, seconds: f64) -> Pressure {
		let queue = fs::read_to_string(&self.path)
			.ok()
			.and_then(|s| s.split_whitespace().nth(8).and_then(|s| s.parse().ok()));
		let total = fs::read_to_string("/proc/pressure/io")
			.ok()
			.and_then(|text| {
				text.lines()
					.find(|line| line.starts_with("full "))
					.and_then(|line| {
						line.split_whitespace()
							.find_map(|field| field.strip_prefix("total=")?.parse::<u64>().ok())
					})
			});
		let stall = total
			.zip(self.previous)
			.filter(|(now, before)| now >= before)
			.map(|(now, before)| {
				((now - before) as f64 / (seconds.max(0.001) * 1_000_000.0)).min(1.0)
			});
		self.previous = total;
		Pressure { queue, stall }
	}
}
#[derive(Default)]
pub(super) struct Samples(VecDeque<f64>);
impl Samples {
	pub fn push(&mut self, ms: f64) {
		if self.0.len() == 128 {
			self.0.pop_front();
		}
		self.0.push_back(ms);
	}
	pub fn p95(&self) -> f64 {
		let mut values: Vec<_> = self.0.iter().copied().collect();
		values.sort_by(f64::total_cmp);
		if values.is_empty() {
			0.0
		} else {
			values[(values.len() - 1) * 95 / 100]
		}
	}
	pub fn clear(&mut self) {
		self.0.clear();
	}
}
pub(super) struct Controller {
	pub depth: usize,
	pub delay: std::time::Duration,
	max: usize,
	stable: u8,
	bad: u8,
	best_rate: f64,
	trial: Option<(usize, f64)>,
}
impl Controller {
	pub fn new(max: usize) -> Self {
		Self {
			depth: 2.min(max),
			delay: std::time::Duration::ZERO,
			max,
			stable: 0,
			bad: 0,
			best_rate: 0.0,
			trial: None,
		}
	}
	pub fn update(
		&mut self,
		rate: f64,
		target_p95: f64,
		flush_p95: f64,
		pressure: Pressure,
		backlog: bool,
		saturated: bool,
	) {
		let busy = target_p95 > 100.0
			|| flush_p95 > 250.0
			|| pressure.stall.is_some_and(|s| s > 0.10)
			|| pressure.queue.is_some_and(|q| q > 32)
			|| backlog;
		if busy {
			self.bad += 1;
			self.stable = 0;
			if self.bad < 2 {
				return;
			}
			self.bad = 0;
			self.depth = (self.depth / 2).max(1);
			self.delay =
				std::time::Duration::from_millis((self.delay.as_millis() as u64 * 2).clamp(5, 100));
			self.stable = 0;
			self.trial = None;
			return;
		}
		self.bad = 0;
		if !saturated || rate <= 0.0 {
			self.stable = 0;
			return;
		}
		self.stable += 1;
		if self.stable < 3 {
			return;
		}
		self.stable = 0;
		if !self.delay.is_zero() {
			self.delay /= 2;
			return;
		}
		if let Some((previous, baseline)) = self.trial.take()
			&& rate < baseline * 1.05
		{
			self.depth = previous;
			self.best_rate = self.best_rate.max(rate);
			return;
		}
		self.best_rate = self.best_rate.max(rate);
		if self.depth < self.max && rate >= self.best_rate * 0.90 {
			self.trial = Some((self.depth, rate));
			self.depth += 1;
		}
	}
}
#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn balanced_depth_increases_slowly_and_halves_under_pressure() {
		let mut c = Controller::new(16);
		for _ in 0..3 {
			c.update(10.0, 1.0, 5.0, Pressure::default(), false, true);
		}
		assert_eq!(c.depth, 3);
		for _ in 0..3 {
			c.update(10.0, 1.0, 5.0, Pressure::default(), false, true);
		}
		assert_eq!(c.depth, 2);
		c.depth = 8;
		c.update(10.0, 200.0, 5.0, Pressure::default(), false, true);
		assert_eq!(c.depth, 8);
		c.update(10.0, 200.0, 5.0, Pressure::default(), false, true);
		assert_eq!(c.depth, 4);
		for _ in 0..8 {
			c.update(10.0, 1.0, 500.0, Pressure::default(), false, true);
		}
		assert_eq!(c.depth, 1);
	}
}
