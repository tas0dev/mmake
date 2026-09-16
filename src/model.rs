use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Project {
    pub variables: HashMap<String, String>,
    pub targets: HashMap<String, Target>,
    pub shell: String,
}

#[derive(Debug, Clone)]
pub struct Target {
    pub name: String,
    pub dependencies: Vec<String>,
    pub watches: Vec<String>,
    pub outputs: Vec<PathBuf>,
    pub always: bool,
    pub recipe: Vec<String>,
}

impl Target {
    pub fn new(name: String, dependencies: Vec<String>) -> Self {
        Self {
            name,
            dependencies,
            watches: Vec::new(),
            outputs: Vec::new(),
            always: false,
            recipe: Vec::new(),
        }
    }
}
