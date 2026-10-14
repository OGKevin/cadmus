//! Callback loop shared by every Slint screen.
//!
//! A screen owns several [`Component`]s. Each one connects its own callbacks,
//! copies its state into the window, and turns one queued action into an
//! effect. The window and the effect are associated types, so a settings
//! screen and a later screen can both use the trait without sharing a window
//! type. The screen reduces that effect to [`super::Flow`], which is what the
//! view loop tells the renderer to do.
//!
//! The future is `!Send` because a screen's state stays on the thread that
//! polls the view tree. Slint's platform and timers are thread-local.

use crate::device::AppContext;
use crate::view::{Bus, Hub};

/// Services a component can use while it handles one callback.
pub(in crate::view) struct Env<'a> {
    pub(in crate::view) hub: &'a Hub,
    pub(in crate::view) bus: &'a mut Bus,
    pub(in crate::view) context: &'a mut AppContext,
}

/// State, callbacks, and effects for one piece of a Slint screen.
#[async_trait::async_trait(?Send)]
pub(in crate::view) trait Component {
    /// Generated window this component binds to.
    type Window;

    /// One callback queued by [`Self::wire`].
    type Action: 'static;

    /// What the screen does with that callback.
    type Effect;

    /// Connects this component's callbacks to its own queue.
    fn wire(&self, window: &Self::Window);

    /// Takes every callback queued since the last drain.
    fn take(&self) -> Vec<Self::Action>;

    /// Pushes this component's state into the window.
    fn sync(&mut self, window: &Self::Window, context: &AppContext);

    /// Handles one queued callback.
    async fn apply(&mut self, action: Self::Action, env: &mut Env<'_>) -> Self::Effect;
}
