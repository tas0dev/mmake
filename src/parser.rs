use anyhow::{Context, Result, bail};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::model::{Project, Target};

pub struct Parser {
    root: PathBuf,
    jobs: usize,
    variables: HashMap<String, String>,
    targets: HashMap<String, Target>,
    shell: String,
    include_stack: Vec<PathBuf>,
}

impl Parser {
    pub fn new(root: PathBuf, jobs: usize) -> Self {
        let mut variables = HashMap::new();
        variables.insert("MMAKE".into(), "1".into());
        variables.insert("ROOT".into(), root.display().to_string());
        variables.insert(
            "CWD".into(),
            std::env::current_dir()
                .unwrap_or_else(|_| root.clone())
                .display()
                .to_string(),
        );
        variables.insert("JOBS".into(), jobs.to_string());

        Self {
            root,
            jobs,
            variables,
            targets: HashMap::new(),
            shell: "/bin/sh".into(),
            include_stack: Vec::new(),
        }
    }

    pub fn parse(mut self, file: &Path, source: &str) -> Result<Project> {
        self.parse_file(file, source)?;

        if let Some(shell) = self.variables.get("SHELL") {
            self.shell = shell.clone();
        }

        self.validate_dependencies()?;

        Ok(Project {
            variables: self.variables,
            targets: self.targets,
            shell: self.shell,
        })
    }

    fn parse_file(&mut self, file: &Path, source: &str) -> Result<()> {
        let canonical = fs::canonicalize(file)
            .with_context(|| format!("could not resolve {}", file.display()))?;

        if self.include_stack.contains(&canonical) {
            bail!("include cycle detected at {}", canonical.display());
        }

        self.include_stack.push(canonical.clone());

        let base_dir = canonical
            .parent()
            .context("Makefile has no parent directory")?
            .to_path_buf();

        let mut current_target: Option<Target> = None;

        for (index, raw_line) in source.lines().enumerate() {
            let line_number = index + 1;

            if raw_line.starts_with('\t') {
                let target = current_target.as_mut().with_context(|| {
                    format!("{}:{}: recipe without target", file.display(), line_number)
                })?;

                let body = raw_line.trim_start_matches('\t');

                if let Some(path) = body.strip_prefix("@watch ") {
                    target.watches.push(self.expand(path.trim())?);
                    continue;
                }

                if let Some(path) = body.strip_prefix("@output ") {
                    let expanded = self.expand(path.trim())?;
                    target.outputs.push(base_dir.join(expanded));
                    continue;
                }

                if body.trim() == "@always" {
                    target.always = true;
                    continue;
                }

                target.recipe.push(self.expand(body)?);
                continue;
            }

            if let Some(target) = current_target.take() {
                self.insert_target(target, file, line_number)?;
            }

            let line = raw_line.trim();

            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            if line.starts_with("ifndef ")
                || line.starts_with("ifdef ")
                || line == "endif"
                || line.starts_with("$(error ")
            {
                continue;
            }

            if let Some(path) = line.strip_prefix("include ") {
                let path = self.expand(path.trim())?;
                let include_path = base_dir.join(path);

                if !include_path.is_file() {
                    bail!(
                        "{}:{}: include not found: {}",
                        file.display(),
                        line_number,
                        include_path.display()
                    );
                }

                let include_source = fs::read_to_string(&include_path)
                    .with_context(|| format!("could not read {}", include_path.display()))?;

                self.parse_file(&include_path, &include_source)?;
                continue;
            }

            if let Some((name, value)) = split_assignment(line, ":=") {
                let value = self.expand(value.trim())?;
                self.variables.insert(name.trim().into(), value);
                continue;
            }

            if let Some((name, value)) = split_assignment(line, "?=") {
                let key = name.trim().to_string();
                if !self.variables.contains_key(&key) {
                    let value = self.expand(value.trim())?;
                    self.variables.insert(key, value);
                }
                continue;
            }

            if let Some((name, value)) = split_assignment(line, "+=") {
                let key = name.trim().to_string();
                let value = self.expand(value.trim())?;
                let entry = self.variables.entry(key).or_default();
                if !entry.is_empty() && !value.is_empty() {
                    entry.push(' ');
                }
                entry.push_str(&value);
                continue;
            }

            if let Some((name, deps)) = line.split_once(':') {
                let name = name.trim();

                if name.is_empty() {
                    bail!("{}:{}: empty target name", file.display(), line_number);
                }

                let dependencies = deps
                    .split_whitespace()
                    .map(|dep| self.expand(dep))
                    .collect::<Result<Vec<_>>>()?;

                current_target = Some(Target::new(name.to_string(), dependencies));
                continue;
            }

            bail!(
                "{}:{}: unsupported syntax: {}",
                file.display(),
                line_number,
                line
            );
        }

        if let Some(target) = current_target.take() {
            self.insert_target(target, file, source.lines().count() + 1)?;
        }

        self.include_stack.pop();
        Ok(())
    }

    fn insert_target(&mut self, target: Target, file: &Path, line: usize) -> Result<()> {
        if self.targets.contains_key(&target.name) {
            bail!(
                "{}:{}: target '{}' is already defined",
                file.display(),
                line,
                target.name
            );
        }

        self.targets.insert(target.name.clone(), target);
        Ok(())
    }

    fn validate_dependencies(&self) -> Result<()> {
        for target in self.targets.values() {
            for dependency in &target.dependencies {
                if !self.targets.contains_key(dependency) {
                    bail!(
                        "target '{}' depends on unknown target '{}'",
                        target.name,
                        dependency
                    );
                }
            }
        }

        let mut visiting = HashSet::new();
        let mut visited = HashSet::new();

        for name in self.targets.keys() {
            self.visit(name, &mut visiting, &mut visited)?;
        }

        Ok(())
    }

    fn visit(
        &self,
        name: &str,
        visiting: &mut HashSet<String>,
        visited: &mut HashSet<String>,
    ) -> Result<()> {
        if visited.contains(name) {
            return Ok(());
        }

        if !visiting.insert(name.to_string()) {
            bail!("dependency cycle detected at target '{}'", name);
        }

        let target = self.targets.get(name).unwrap();

        for dependency in &target.dependencies {
            self.visit(dependency, visiting, visited)?;
        }

        visiting.remove(name);
        visited.insert(name.to_string());

        Ok(())
    }

    fn expand(&self, input: &str) -> Result<String> {
        let mut output = String::new();
        let bytes = input.as_bytes();
        let mut i = 0;

        while i < bytes.len() {
            if i + 1 < bytes.len() && bytes[i] == b'$' && bytes[i + 1] == b'(' {
                let start = i + 2;
                let mut end = start;

                while end < bytes.len() && bytes[end] != b')' {
                    end += 1;
                }

                if end >= bytes.len() {
                    bail!("unterminated variable reference in '{}'", input);
                }

                let name = &input[start..end];
                let value = self
                    .variables
                    .get(name)
                    .with_context(|| format!("undefined variable '{}'", name))?;

                output.push_str(value);
                i = end + 1;
                continue;
            }

            output.push(bytes[i] as char);
            i += 1;
        }

        Ok(output)
    }
}

fn split_assignment<'a>(line: &'a str, operator: &str) -> Option<(&'a str, &'a str)> {
    let position = line.find(operator)?;
    let left = &line[..position];

    if left.contains(':') {
        return None;
    }

    Some((&line[..position], &line[position + operator.len()..]))
}
