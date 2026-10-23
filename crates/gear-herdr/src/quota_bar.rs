//! Quota readings from a CLI's footer, never from its conversation tail.

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuotaBar {
    /// The last nonempty screen line must begin with this CLI footer.
    pub footer_prefix: String,
    /// The input separator immediately above the status bar.
    pub separator_prefix: String,
    /// Usage segments, e.g. `5h 100% ↻01:10` and `7d 100% ↻Oct23`.
    pub windows: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaReading {
    pub exhausted: bool,
    /// One reset token per exhausted window; missing tokens use the policy fallback.
    pub resets: Vec<Option<String>>,
    /// Only the exhausted usage segments, excluding model/context changes.
    pub signature: String,
}

impl QuotaBar {
    pub fn claude() -> Self {
        Self {
            footer_prefix: "⏵⏵ bypass permissions on".into(),
            separator_prefix: "─".into(),
            windows: vec!["5h".into(), "7d".into()],
        }
    }

    pub fn read(&self, screen: &str) -> Option<QuotaReading> {
        if self.footer_prefix.is_empty() || self.separator_prefix.is_empty() {
            return None;
        }
        let lines: Vec<_> = screen
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect();
        let footer = lines.len().checked_sub(1)?;
        if !lines[footer].trim_start().starts_with(&self.footer_prefix) {
            return None;
        }
        // Read only the status region between the bottom input separator and
        // the permission footer. A quoted bar further up is never inspected.
        let start = (footer.saturating_sub(4)..footer).rev().find(|&i| {
            let line = lines[i].trim();
            line.starts_with(&self.separator_prefix)
                && line.chars().all(|c| self.separator_prefix.contains(c))
        })?;
        let bar = lines[start + 1..footer].join(" ");
        let mut found = false;
        let mut exhausted = Vec::new();
        let mut resets = Vec::new();
        for segment in bar.split('·').map(str::trim) {
            let mut words = segment.split_whitespace();
            let Some(window) = words.next() else { continue };
            if !self
                .windows
                .iter()
                .any(|label| !label.is_empty() && label == window)
            {
                continue;
            }
            let Some(percent) = words
                .next()
                .and_then(|word| word.strip_suffix('%'))
                .and_then(|number| number.parse::<u32>().ok())
                .filter(|n| *n <= 100)
            else {
                continue;
            };
            found = true;
            // The verified Claude bar displays used_percentage, not remaining.
            if percent == 100 {
                resets.push(words.find_map(|word| word.strip_prefix('↻').map(str::to_string)));
                exhausted.push(segment.split_whitespace().collect::<Vec<_>>().join(" "));
            }
        }
        found.then(|| QuotaReading {
            exhausted: !exhausted.is_empty(),
            resets,
            signature: exhausted.join(" · "),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(bar: &str) -> String {
        format!("quota exhausted in the conversation\n────────────────\n❯\n────────────────\n  Opus 5.5 (1M context) · medium /repo (dev)  ·  ctx 63%  ·  {bar}\n  ⏵⏵ bypass permissions on (shift+tab to cycle)\n")
    }

    #[test]
    fn only_the_actual_footer_is_read() {
        let profile = QuotaBar::claude();
        let quoted = format!("{}\n{}", screen("5h 100% ↻01:10"), screen("5h 4% ↻16:20"));
        assert!(!profile.read(&quoted).unwrap().exhausted);
        assert!(profile.read("the bar says 5h 100% ↻01:10").is_none());
        assert!(profile
            .read("5h 100% ↻01:10\n⏵⏵ bypass permissions on")
            .is_none());
    }

    #[test]
    fn the_bar_reports_used_percent_and_the_exhausted_windows_reset() {
        let profile = QuotaBar::claude();
        assert!(!profile.read(&screen("5h 1% ↻01:10")).unwrap().exhausted);
        let reading = profile
            .read(&screen("5h 100% ↻01:10 · 7d 73% ↻Oct23"))
            .unwrap();
        assert!(reading.exhausted);
        assert_eq!(reading.resets, vec![Some("01:10".into())]);
        let reading = profile.read(&screen("5h 100% · 7d 100% ↻Oct23")).unwrap();
        assert_eq!(reading.resets, vec![None, Some("Oct23".into())]);
    }
}
