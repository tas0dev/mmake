use anyhow::{Context, Result, anyhow, bail};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;

use crate::cache::{Cache, TargetState};
use crate::fingerprint::{self, FingerprintResult};
use crate::model::{Project, Target};
use crate::progress::Progress;

const STEP_MARKER: &[u8] = b"\x1eMMAKE_STEP\x1f";

struct Job {
    name: String,
    target: Target,
    fingerprint: FingerprintResult,
}

struct JobResult {
    name: String,
    fingerprint: FingerprintResult,
    result: Result<()>,
}

enum WorkerEvent {
    StepDone { name: String },
    Finished(JobResult),
}

pub struct Executor {
    project: Project,
    root: PathBuf,
    jobs: usize,
    dry_run: bool,
    verbose: bool,
    total_commands: usize,
    finished_commands: usize,
    cache: Cache,
    cache_path: PathBuf,
    fingerprints: HashMap<String, String>,
    progress: Option<Progress>,
}

impl Executor {
    pub fn new(project: Project, root: PathBuf, jobs: usize, dry_run: bool, verbose: bool) -> Self {
        let cache_path = root.join(".mmake/state.json");

        let cache = Cache::load(&cache_path).unwrap_or_default();

        Self {
            project,
            root,
            jobs,
            dry_run,
            verbose,
            total_commands: 0,
            finished_commands: 0,
            cache,
            cache_path,
            fingerprints: HashMap::new(),
            progress: None,
        }
    }

    pub fn run(&mut self, target: &str) -> Result<()> {
        let targets = self.collect_targets(target)?;

        self.total_commands = targets
            .iter()
            .filter_map(|name| self.project.targets.get(name))
            .map(|target| target.recipe.len())
            .sum();

        self.progress = Some(Progress::new(self.total_commands));

        if let Some(progress) = self.progress.as_mut() {
            progress.draw()?;
        }

        let result = self.run_scheduled_targets(targets);

        if let Some(progress) = self.progress.as_ref() {
            progress.finish()?;
        }

        result
    }

    fn run_scheduled_targets(&mut self, targets: HashSet<String>) -> Result<()> {
        let mut dependency_count = HashMap::<String, usize>::new();
        let mut dependents = HashMap::<String, Vec<String>>::new();

        for name in &targets {
            let item = self
                .project
                .targets
                .get(name)
                .with_context(|| format!("unknown target '{}'", name))?;

            let count = item
                .dependencies
                .iter()
                .filter(|dependency| targets.contains(*dependency))
                .count();

            dependency_count.insert(name.clone(), count);

            for dependency in &item.dependencies {
                if !targets.contains(dependency) {
                    continue;
                }

                dependents
                    .entry(dependency.clone())
                    .or_default()
                    .push(name.clone());
            }
        }

        let mut ready = VecDeque::new();

        for name in &targets {
            if dependency_count.get(name).copied().unwrap_or(0) == 0 {
                ready.push_back(name.clone());
            }
        }

        let (sender, receiver) = mpsc::channel::<WorkerEvent>();

        let mut running = HashSet::<String>::new();
        let mut completed = HashSet::<String>::new();
        let mut failed: Option<anyhow::Error> = None;

        while completed.len() < targets.len() {
            while failed.is_none() && running.len() < self.jobs && !ready.is_empty() {
                let name = ready.pop_front().unwrap();

                if completed.contains(&name) || running.contains(&name) {
                    continue;
                }

                let target = self
                    .project
                    .targets
                    .get(&name)
                    .with_context(|| format!("unknown target '{}'", name))?
                    .clone();

                let dependency_fingerprints = target
                    .dependencies
                    .iter()
                    .filter_map(|dependency| self.fingerprints.get(dependency).cloned())
                    .collect::<Vec<_>>();

                let previous_files = self.cache.targets.get(&name).map(|state| &state.files);

                let fingerprint_result = fingerprint::calculate(
                    &self.root,
                    &target,
                    previous_files,
                    &dependency_fingerprints,
                )?;

                let outputs_exist = target.outputs.iter().all(|output| output.exists());

                let fingerprint_matches = self
                    .cache
                    .targets
                    .get(&name)
                    .map(|state| state.fingerprint == fingerprint_result.fingerprint)
                    .unwrap_or(false);

                if !target.always && fingerprint_matches && outputs_exist {
                    if let Some(progress) = self.progress.as_mut() {
                        progress.log_skip(&name)?;
                    }

                    self.fingerprints
                        .insert(name.clone(), fingerprint_result.fingerprint.clone());

                    self.finished_commands += target.recipe.len();

                    if let Some(progress) = self.progress.as_mut() {
                        progress.set_finished(self.finished_commands)?;
                    }

                    completed.insert(name.clone());

                    self.release_dependents(
                        &name,
                        &mut dependency_count,
                        &dependents,
                        &completed,
                        &running,
                        &mut ready,
                    );

                    continue;
                }

                if target.recipe.is_empty() {
                    self.record_target_state(&name, fingerprint_result);

                    completed.insert(name.clone());

                    self.release_dependents(
                        &name,
                        &mut dependency_count,
                        &dependents,
                        &completed,
                        &running,
                        &mut ready,
                    );

                    continue;
                }

                if let Some(progress) = self.progress.as_mut() {
                    progress.log_build(&name)?;
                }

                if self.dry_run || self.verbose {
                    for command in &target.recipe {
                        println!("  {}", command);
                    }
                }

                if self.dry_run {
                    self.fingerprints
                        .insert(name.clone(), fingerprint_result.fingerprint);

                    self.finished_commands += target.recipe.len();

                    if let Some(progress) = self.progress.as_mut() {
                        progress.set_finished(self.finished_commands)?;
                    }

                    completed.insert(name.clone());

                    self.release_dependents(
                        &name,
                        &mut dependency_count,
                        &dependents,
                        &completed,
                        &running,
                        &mut ready,
                    );

                    continue;
                }

                let job = Job {
                    name: name.clone(),
                    target,
                    fingerprint: fingerprint_result,
                };

                self.spawn_job(job, sender.clone());

                running.insert(name);
            }

            if running.is_empty() {
                if failed.is_some() {
                    break;
                }

                if completed.len() == targets.len() {
                    break;
                }

                bail!("build scheduler reached a deadlock");
            }

            let event = receiver
                .recv()
                .context("build worker channel closed unexpectedly")?;

            match event {
                WorkerEvent::StepDone { name: _ } => {
                    self.finished_commands += 1;

                    if let Some(progress) = self.progress.as_mut() {
                        progress.set_finished(self.finished_commands)?;
                    }
                }

                WorkerEvent::Finished(job_result) => {
                    running.remove(&job_result.name);

                    match job_result.result {
                        Ok(()) => {
                            if let Some(progress) = self.progress.as_mut() {
                                progress.log_done(&job_result.name)?;
                            }

                            self.record_target_state(&job_result.name, job_result.fingerprint);

                            completed.insert(job_result.name.clone());

                            self.release_dependents(
                                &job_result.name,
                                &mut dependency_count,
                                &dependents,
                                &completed,
                                &running,
                                &mut ready,
                            );
                        }

                        Err(error) => {
                            if let Some(progress) = self.progress.as_mut() {
                                progress.log_fail(&job_result.name)?;
                            }

                            if failed.is_none() {
                                failed = Some(error);
                            }
                        }
                    }
                }
            }
        }

        while !running.is_empty() {
            let event = receiver
                .recv()
                .context("build worker channel closed unexpectedly")?;

            match event {
                WorkerEvent::StepDone { name: _ } => {
                    self.finished_commands += 1;

                    if let Some(progress) = self.progress.as_mut() {
                        progress.set_finished(self.finished_commands)?;
                    }
                }

                WorkerEvent::Finished(job_result) => {
                    running.remove(&job_result.name);

                    match job_result.result {
                        Ok(()) => {
                            if let Some(progress) = self.progress.as_mut() {
                                progress.log_done(&job_result.name)?;
                            }

                            self.record_target_state(&job_result.name, job_result.fingerprint);
                        }

                        Err(error) => {
                            if let Some(progress) = self.progress.as_mut() {
                                progress.log_fail(&job_result.name)?;
                            }

                            if failed.is_none() {
                                failed = Some(error);
                            }
                        }
                    }
                }
            }
        }

        if !self.dry_run {
            self.cache.save(&self.cache_path)?;
        }

        if let Some(error) = failed {
            return Err(error);
        }

        Ok(())
    }

    fn collect_targets(&self, target: &str) -> Result<HashSet<String>> {
        let mut targets = HashSet::new();
        let mut visiting = HashSet::new();

        self.collect_target_recursive(target, &mut targets, &mut visiting)?;

        Ok(targets)
    }

    fn collect_target_recursive(
        &self,
        name: &str,
        targets: &mut HashSet<String>,
        visiting: &mut HashSet<String>,
    ) -> Result<()> {
        if targets.contains(name) {
            return Ok(());
        }

        if !visiting.insert(name.to_string()) {
            bail!("dependency cycle detected at target '{}'", name);
        }

        let target = self
            .project
            .targets
            .get(name)
            .with_context(|| format!("unknown target '{}'", name))?;

        for dependency in &target.dependencies {
            self.collect_target_recursive(dependency, targets, visiting)?;
        }

        visiting.remove(name);
        targets.insert(name.to_string());

        Ok(())
    }

    fn release_dependents(
        &self,
        name: &str,
        dependency_count: &mut HashMap<String, usize>,
        dependents: &HashMap<String, Vec<String>>,
        completed: &HashSet<String>,
        running: &HashSet<String>,
        ready: &mut VecDeque<String>,
    ) {
        let Some(items) = dependents.get(name) else {
            return;
        };

        for dependent in items {
            let Some(count) = dependency_count.get_mut(dependent) else {
                continue;
            };

            if *count > 0 {
                *count -= 1;
            }

            if *count == 0
                && !completed.contains(dependent)
                && !running.contains(dependent)
                && !ready.contains(dependent)
            {
                ready.push_back(dependent.clone());
            }
        }
    }

    fn spawn_job(&self, job: Job, sender: mpsc::Sender<WorkerEvent>) {
        let root = self.root.clone();
        let shell = self.project.shell.clone();
        let jobs = self.jobs;
        let verbose = self.verbose;

        thread::spawn(move || {
            let result = execute_job(&root, &shell, jobs, verbose, &job.target, sender.clone());

            let event = WorkerEvent::Finished(JobResult {
                name: job.name,
                fingerprint: job.fingerprint,
                result,
            });

            let _ = sender.send(event);
        });
    }

    fn record_target_state(&mut self, name: &str, fingerprint_result: FingerprintResult) {
        self.fingerprints
            .insert(name.to_string(), fingerprint_result.fingerprint.clone());

        if self.dry_run {
            return;
        }

        self.cache.targets.insert(
            name.to_string(),
            TargetState {
                fingerprint: fingerprint_result.fingerprint,
                files: fingerprint_result.files,
            },
        );
    }
}

fn execute_job(
    root: &Path,
    shell: &str,
    jobs: usize,
    verbose: bool,
    target: &Target,
    sender: mpsc::Sender<WorkerEvent>,
) -> Result<()> {
    let script = build_instrumented_script(&target.recipe);

    let mut child = Command::new(shell)
        .arg("-eu")
        .arg("-c")
        .arg(&script)
        .current_dir(root)
        .env("MMAKE", "1")
        .env("MMAKE_JOBS", jobs.to_string())
        .env("JOBS", jobs.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to execute target '{}'", target.name))?;

    let stdout = child
        .stdout
        .take()
        .context("could not capture child stdout")?;

    let stderr = child
        .stderr
        .take()
        .context("could not capture child stderr")?;

    let stdout_target = target.name.clone();
    let stdout_sender = sender.clone();

    let stdout_thread =
        thread::spawn(move || read_stdout(stdout, verbose, stdout_target, stdout_sender));

    let stderr_thread = thread::spawn(move || read_stderr(stderr, verbose));

    let status = child.wait().context("could not wait for build process")?;

    let stdout = stdout_thread
        .join()
        .map_err(|_| anyhow!("stdout reader panicked"))??;

    let stderr = stderr_thread
        .join()
        .map_err(|_| anyhow!("stderr reader panicked"))??;

    if !status.success() {
        let stdout = String::from_utf8_lossy(&stdout);

        let stderr = String::from_utf8_lossy(&stderr);

        let mut message = format!("target '{}' failed with status {}", target.name, status);

        if !stdout.trim().is_empty() {
            message.push_str("\n\nstdout:\n");
            message.push_str(&stdout);
        }

        if !stderr.trim().is_empty() {
            message.push_str("\n\nstderr:\n");
            message.push_str(&stderr);
        }

        return Err(anyhow!(message));
    }

    for output in &target.outputs {
        if !output.exists() {
            bail!(
                "target '{}' completed successfully, but output was not produced: {}",
                target.name,
                output.display()
            );
        }
    }

    Ok(())
}

fn build_instrumented_script(recipe: &[String]) -> String {
    let mut script = String::new();

    for command in recipe {
        script.push_str(command);
        script.push('\n');

        script.push_str("printf '\\036MMAKE_STEP\\037'\n");
    }

    script
}

fn read_stdout<R>(
    mut reader: R,
    verbose: bool,
    target_name: String,
    sender: mpsc::Sender<WorkerEvent>,
) -> Result<Vec<u8>>
where
    R: Read,
{
    let mut captured = Vec::new();
    let mut pending = Vec::new();
    let mut buffer = [0u8; 4096];

    loop {
        let count = reader.read(&mut buffer)?;

        if count == 0 {
            break;
        }

        pending.extend_from_slice(&buffer[..count]);

        loop {
            let Some(position) = find_bytes(&pending, STEP_MARKER) else {
                break;
            };

            let output = pending[..position].to_vec();

            write_visible_stdout(&output, verbose)?;

            captured.extend_from_slice(&output);

            pending.drain(..position + STEP_MARKER.len());

            let _ = sender.send(WorkerEvent::StepDone {
                name: target_name.clone(),
            });
        }

        if pending.len() > STEP_MARKER.len() {
            let keep = STEP_MARKER.len() - 1;
            let flush = pending.len() - keep;

            let output = pending[..flush].to_vec();

            write_visible_stdout(&output, verbose)?;

            captured.extend_from_slice(&output);

            pending.drain(..flush);
        }
    }

    if !pending.is_empty() {
        write_visible_stdout(&pending, verbose)?;

        captured.extend_from_slice(&pending);
    }

    Ok(captured)
}

fn read_stderr<R>(mut reader: R, verbose: bool) -> Result<Vec<u8>>
where
    R: Read,
{
    let mut captured = Vec::new();
    let mut buffer = [0u8; 4096];

    loop {
        let count = reader.read(&mut buffer)?;

        if count == 0 {
            break;
        }

        let output = &buffer[..count];

        if verbose {
            let stderr = std::io::stderr();
            let mut stderr = stderr.lock();

            stderr.write_all(output)?;
            stderr.flush()?;
        }

        captured.extend_from_slice(output);
    }

    Ok(captured)
}

fn write_visible_stdout(output: &[u8], verbose: bool) -> Result<()> {
    if !verbose || output.is_empty() {
        return Ok(());
    }

    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();

    stdout.write_all(output)?;
    stdout.flush()?;

    Ok(())
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }

    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
