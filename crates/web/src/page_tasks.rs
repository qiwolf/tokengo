//! Page-owned reads publish independently and stop updating after navigation.
use leptos::prelude::*;
use std::{future::Future, pin::Pin};

#[derive(Clone, Copy)]
pub(crate) struct PageTasks(StoredValue<Owner>);
impl PageTasks {
    pub(crate) fn new() -> Self {
        Self(StoredValue::new(Owner::current().expect("page owner")))
    }
    pub(crate) fn spawn(self, task: impl Future<Output = ()> + 'static) {
        self.0.with_value(|owner| owner.with(|| {
            leptos::task::spawn_local_scoped_with_cancellation(task);
        }));
    }
}

pub(crate) type ReadTask = Pin<Box<dyn Future<Output = ()>>>;
/// Each branch applies its result before waiting for the other branches.
pub(crate) async fn parallel<const N: usize>(tasks: [ReadTask; N]) {
    futures_util::future::join_all(tasks).await;
}

pub(crate) fn foreground() -> bool {
    web_sys::window().and_then(|w| w.document()).is_none_or(|d| !d.hidden())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, rc::Rc, task::{Context, Poll}};
    #[test]
    fn ready_section_publishes_while_another_read_is_pending() {
        let published = Rc::new(Cell::new(false));
        let done = published.clone();
        let mut work = Box::pin(parallel([
            Box::pin(std::future::pending()) as ReadTask,
            Box::pin(async move { done.set(true); }) as ReadTask,
        ]));
        let mut cx = Context::from_waker(futures_util::task::noop_waker_ref());
        assert!(matches!(work.as_mut().poll(&mut cx), Poll::Pending));
        assert!(published.get());
    }
    #[test]
    fn dropping_page_work_drops_pending_reads() {
        struct Pending(Rc<Cell<bool>>);
        impl Future for Pending {
            type Output = ();
            fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> { Poll::Pending }
        }
        impl Drop for Pending { fn drop(&mut self) { self.0.set(true); } }
        let dropped = Rc::new(Cell::new(false));
        let mut work = Box::pin(parallel([Box::pin(Pending(dropped.clone())) as ReadTask]));
        let mut cx = Context::from_waker(futures_util::task::noop_waker_ref());
        assert!(work.as_mut().poll(&mut cx).is_pending());
        drop(work);
        assert!(dropped.get());
    }
}
