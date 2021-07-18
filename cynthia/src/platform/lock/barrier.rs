use crate::platform::event::Event;
use crate::platform::lock::Mutex;

#[derive(Debug, Clone)]
pub struct BarrierWaitResult {
    is_leader: bool,
}

impl BarrierWaitResult {
    pub fn is_leader(&self) -> bool {
        self.is_leader
    }
}

#[derive(Debug)]
pub struct Barrier {
    n: usize,
    state: Mutex<State>,
    event: Event,
}

#[derive(Debug)]
struct State {
    count: usize,
    generation_id: u64,
}

impl Barrier {
    pub const fn new(n: usize) -> Barrier {
        Barrier {
            n,
            state: Mutex::new(State {
                count: 0,
                generation_id: 0,
            }),
            event: Event::new(),
        }
    }

    pub async fn wait(&self) -> BarrierWaitResult {
        let mut state = self.state.lock().await;
        let local_gen = state.generation_id;
        state.count += 1;

        if state.count < self.n {
            while local_gen == state.generation_id && state.count < self.n {
                let listener = self.event.listen();
                drop(state);
                listener.await;
                state = self.state.lock().await;
            }
            BarrierWaitResult { is_leader: false }
        } else {
            state.count = 0;
            state.generation_id = state.generation_id.wrapping_add(1);
            self.event.notify(std::usize::MAX);
            BarrierWaitResult { is_leader: true }
        }
    }
}
