use std::sync::OnceLock;

pub struct TaskbarMediaConfig {
    pub priority_list: Vec<String>,
    pub size: [f32; 2],
    pub position: [f32; 2],
}

impl TaskbarMediaConfig {
    pub fn new(priority_list: Vec<String>, size: [f32; 2], position: [f32; 2]) -> Self {
        Self { priority_list, size, position }
    }
}

static CONFIG: OnceLock<TaskbarMediaConfig> = OnceLock::new();

pub fn init_config(config: TaskbarMediaConfig) -> Result<(), &'static str> {
    CONFIG
        .set(config)
        .map_err(|_| "Config has already been initialized")
}

pub fn get_config() -> Option<&'static TaskbarMediaConfig> {
    CONFIG.get()
}