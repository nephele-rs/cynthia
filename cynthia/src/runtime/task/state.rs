pub const SCHEDULED: usize = 1 << 0;
pub const RUNNING: usize = 1 << 1;
pub const COMPLETED: usize = 1 << 2;
pub const CLOSED: usize = 1 << 3;

pub const TASK: usize = 1 << 4;
pub const AWAITER: usize = 1 << 5;
pub const REGISTERING: usize = 1 << 6;
pub const NOTIFYING: usize = 1 << 7;
pub const REFERENCE: usize = 1 << 8;
