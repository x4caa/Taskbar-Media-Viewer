pub struct TaskbarMediaConfig {
    pub priority_list: Vec<String>,
}

impl TaskbarMediaConfig {
    pub fn new(priority_list: Vec<String>) -> Self {
        Self { priority_list }
    }
}