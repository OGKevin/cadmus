//! Defers actions from Slint callbacks to the view that owns them.

use std::cell::RefCell;
use std::rc::Rc;

/// Actions queued by callbacks for a view to process after dispatch.
pub(crate) struct CallbackQueue<T> {
    actions: Rc<RefCell<Vec<T>>>,
}

impl<T> CallbackQueue<T> {
    pub(crate) fn new() -> Self {
        Self {
            actions: Rc::new(RefCell::new(Vec::new())),
        }
    }

    /// Creates a handle that a `'static` Slint callback can use to queue actions.
    pub(crate) fn handle(&self) -> CallbackHandle<T> {
        CallbackHandle {
            actions: Rc::clone(&self.actions),
        }
    }

    /// Takes every action queued so far.
    pub(crate) fn drain(&self) -> Vec<T> {
        std::mem::take(&mut *self.actions.borrow_mut())
    }
}

/// Handle used by a Slint callback to queue an action.
#[derive(Clone)]
pub(crate) struct CallbackHandle<T> {
    actions: Rc<RefCell<Vec<T>>>,
}

impl<T> CallbackHandle<T> {
    pub(crate) fn push(&self, action: T) {
        self.actions.borrow_mut().push(action);
    }
}

/// Connects a Slint callback to a [`CallbackQueue`].
macro_rules! connect {
    ($ui:expr, $queue:expr, $setter:ident, ($($arg:ident),*) => $action:expr) => {{
        let queued = $queue.handle();
        $ui.$setter(move |$($arg),*| queued.push($action));
    }};
}
pub(crate) use connect;
