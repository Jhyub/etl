use std::{
    io::{self, IsTerminal},
    time::Duration,
};

use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};

pub struct Progress(ProgressBar);

impl Progress {
    pub fn new(message: String) -> Self {
        let target = if io::stderr().is_terminal() {
            ProgressDrawTarget::stderr()
        } else {
            ProgressDrawTarget::hidden()
        };
        let bar = ProgressBar::with_draw_target(None, target);
        bar.set_style(
            ProgressStyle::with_template("{spinner} {prefix} {bytes}")
                .expect("valid progress template"),
        );
        bar.set_prefix(message);
        bar.enable_steady_tick(Duration::from_millis(100));
        Self(bar)
    }

    pub fn transfer(&self, size: Option<u64>) {
        if size.is_some_and(|size| size > 0) {
            self.0.set_length(1000);
            self.0.set_style(
                ProgressStyle::with_template("{spinner} {prefix} [{wide_bar}] {percent}% {msg}")
                    .expect("valid progress template"),
            );
        }
    }

    pub fn bar(&self) -> ProgressBar {
        self.0.clone()
    }

    pub fn label(&self, message: String) {
        self.0.set_prefix(message);
    }

    pub fn update(bar: &ProgressBar, size: Option<u64>, bytes: u64) {
        if let Some(size) = size.filter(|size| *size > 0) {
            let position = ((u128::from(bytes) * 1000 / u128::from(size)).min(999)) as u64;
            bar.set_position(position);
            bar.set_message(format!("{bytes}/{size} bytes"));
        } else {
            bar.set_position(bytes);
        }
    }

    pub fn finish(self, message: String) {
        self.0.set_length(1000);
        self.0.set_style(
            ProgressStyle::with_template("{msg} [{wide_bar}] {percent}%")
                .expect("valid progress template"),
        );
        self.0.set_position(1000);
        self.0.finish_with_message(message);
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        if !self.0.is_finished() {
            self.0.abandon_with_message("Failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_waits_for_success() {
        let progress = Progress::new("file".into());
        progress.transfer(Some(10));
        let bar = progress.bar();
        Progress::update(&bar, Some(10), 10);
        assert!(bar.position() < 1000);
        progress.finish("saved".into());
        assert_eq!(bar.position(), 1000);
    }

    #[test]
    fn failure_does_not_complete_bar() {
        let progress = Progress::new("file".into());
        progress.transfer(Some(10));
        let bar = progress.bar();
        Progress::update(&bar, Some(10), 5);
        drop(progress);
        assert!(bar.position() < 1000);
    }

    #[test]
    fn unknown_length_tracks_bytes_until_finished() {
        let progress = Progress::new("file".into());
        progress.transfer(None);
        let bar = progress.bar();
        Progress::update(&bar, None, 42);
        assert_eq!(bar.position(), 42);
        progress.finish("saved".into());
        assert_eq!(bar.position(), 1000);
    }
}
