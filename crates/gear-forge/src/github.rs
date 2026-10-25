//! The forge on GitHub (T-78), through the `gh` CLI.
//!
//! Every call names its repo (`-R owner/name`), asks for JSON, and passes
//! nothing on stdin: `gh` never prompts, and an unexpected prompt fails at
//! once instead of hanging a lane. The environment turns the update notifier
//! and paging off too, so the answer is the answer.
//!
//! Every call is bounded: past the timeout the child is killed and the call
//! answers [`Error::Timeout`]. Output is read on its own thread while the
//! call runs, so a large answer cannot fill the pipe and deadlock the child.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::{Error, Filter, Forge, ForgeResult, Issue, MergeMethod, MergeResult, PrRef};

/// How long one `gh` call may take before it is killed.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// How often the call checks whether the child has finished.
const POLL: Duration = Duration::from_millis(20);

/// GitHub, through `gh` (T-78).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHub {
    /// The `PATH` to find `gh` in; `None` = the inherited one. A test points
    /// it at the directory that holds its fake `gh`, so no test can reach a
    /// real one.
    path: Option<PathBuf>,
    timeout: Duration,
}

impl GitHub {
    /// The `gh` of `PATH`, with [`DEFAULT_TIMEOUT`].
    pub fn new() -> GitHub {
        GitHub {
            path: None,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Look for `gh` in `dir` instead of the inherited `PATH` (a test's fake
    /// `gh`).
    pub fn with_path(mut self, dir: impl Into<PathBuf>) -> GitHub {
        self.path = Some(dir.into());
        self
    }

    /// How long one call may take.
    pub fn with_timeout(mut self, timeout: Duration) -> GitHub {
        self.timeout = timeout;
        self
    }

    /// The command for one call: explicit, silent, and never paging.
    fn command(&self, args: &[String]) -> Command {
        let mut command = Command::new("gh");
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // A prompt in an unattended call fails instead of waiting.
            .env("GH_PROMPT_DISABLED", "1")
            // The update notifier writes to stderr and costs seconds.
            .env("GH_NO_UPDATE_NOTIFIER", "1")
            .env("GH_PAGER", "cat");
        if let Some(dir) = &self.path {
            command.env("PATH", dir);
        }
        command
    }

    /// Run one `gh` call and answer its stdout, or a typed error.
    fn run(&self, args: &[String]) -> ForgeResult<String> {
        let what = format!("gh {}", args.join(" "));
        let mut child = self
            .command(args)
            .spawn()
            .map_err(|e| Error::Tool(format!("{what}: {e}")))?;
        // Read both pipes off the call's thread: a big answer must not fill
        // the pipe while we wait for the child to finish.
        let mut stdout = child.stdout.take();
        let mut stderr = child.stderr.take();
        let (sender, reader) = mpsc::channel();
        std::thread::spawn(move || {
            let mut out = String::new();
            let mut err = String::new();
            if let Some(stdout) = stdout.as_mut() {
                let _ = stdout.read_to_string(&mut out);
            }
            if let Some(stderr) = stderr.as_mut() {
                let _ = stderr.read_to_string(&mut err);
            }
            let _ = sender.send((out, err));
        });

        let deadline = Instant::now() + self.timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {
                    if Instant::now() >= deadline {
                        // The bound is the point: a forge that hangs must not
                        // hold a lane.
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(Error::Timeout(self.timeout));
                    }
                    std::thread::sleep(POLL);
                }
                Err(e) => return Err(Error::Tool(format!("{what}: {e}"))),
            }
        };
        let (out, err) = reader
            .recv()
            .map_err(|_| Error::Tool(format!("{what}: its output could not be read")))?;
        if !status.success() {
            let said = err.trim();
            let said = if said.is_empty() { out.trim() } else { said };
            return Err(Error::Failed(format!("{what}: {said}")));
        }
        Ok(out)
    }
}

impl Forge for GitHub {
    fn issues(&self, repo: &str, filter: &Filter) -> ForgeResult<Vec<Issue>> {
        let mut args: Vec<String> = vec![
            "issue".into(),
            "list".into(),
            "--repo".into(),
            repo.into(),
            "--state".into(),
            "open".into(),
            "--json".into(),
            "number,title,body,labels".into(),
        ];
        if filter.limit > 0 {
            args.push("--limit".into());
            args.push(filter.limit.to_string());
        }
        for label in &filter.labels {
            args.push("--label".into());
            args.push(label.clone());
        }
        let out = self.run(&args)?;
        serde_json::from_str::<Vec<Issue>>(&out)
            .map_err(|e| Error::Unreadable(format!("the issue list: {e}")))
    }

    fn open_claim(
        &self,
        repo: &str,
        branch: &str,
        base: &str,
        title: &str,
        body: &str,
    ) -> ForgeResult<PrRef> {
        let args: Vec<String> = vec![
            "pr".into(),
            "create".into(),
            "--repo".into(),
            repo.into(),
            "--head".into(),
            branch.into(),
            "--base".into(),
            base.into(),
            "--title".into(),
            title.into(),
            "--body".into(),
            body.into(),
            // A claim is a draft: it says "this lane took the issue", not
            // "this is ready to merge".
            "--draft".into(),
        ];
        // `gh pr create` has no `--json`: on success it prints the new pull
        // request's URL, and the number is the URL's last segment.
        let out = self.run(&args)?;
        let url = out
            .lines()
            .map(str::trim)
            .find(|line| line.starts_with("http"))
            .ok_or_else(|| Error::Unreadable(format!("the opened pull request: {out:?}")))?;
        let number = url
            .rsplit('/')
            .next()
            .and_then(|last| last.parse::<u64>().ok())
            .ok_or_else(|| Error::Unreadable(format!("the number in {url}")))?;
        Ok(PrRef {
            repo: repo.to_string(),
            number,
            url: url.to_string(),
        })
    }

    fn comment(&self, pr: &PrRef, body: &str) -> ForgeResult<()> {
        if !pr.is_open() {
            // A forge that opened nothing has nothing to comment on.
            return Ok(());
        }
        let args: Vec<String> = vec![
            "pr".into(),
            "comment".into(),
            pr.number.to_string(),
            "--repo".into(),
            pr.repo.clone(),
            "--body".into(),
            body.into(),
        ];
        self.run(&args).map(|_| ())
    }

    fn merge(&self, pr: &PrRef, method: MergeMethod) -> ForgeResult<MergeResult> {
        if !pr.is_open() {
            return Ok(MergeResult {
                merged: false,
                message: "no pull request to merge".to_string(),
            });
        }
        let args: Vec<String> = vec![
            "pr".into(),
            "merge".into(),
            pr.number.to_string(),
            "--repo".into(),
            pr.repo.clone(),
            method.flag().into(),
        ];
        let out = self.run(&args)?;
        Ok(MergeResult {
            merged: true,
            message: out.trim().to_string(),
        })
    }

    fn close_issue(&self, repo: &str, number: u64, comment: &str) -> ForgeResult<()> {
        let args: Vec<String> = vec![
            "issue".into(),
            "close".into(),
            number.to_string(),
            "--repo".into(),
            repo.into(),
            "--comment".into(),
            comment.into(),
        ];
        self.run(&args).map(|_| ())
    }
}
