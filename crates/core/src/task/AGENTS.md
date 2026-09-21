# Background tasks (`task`)

[`TaskManager`](mod.rs) stop paths must use the same join policy:

- [`TaskManager::stop`](mod.rs) and [`TaskManager::stop_all`](mod.rs) are `async`
  and join spawned tasks with **`TASK_STOP_DEADLINE` (5 seconds)** per task.
  `stop_sync` no longer exists: callers `.await` the stop path. Never
  `runtime::block_on` a join from an `async` context; a parking `block_on`
  inside a two-worker runtime stalls input and timers.
- When you change one stop path, update the other and keep the shared deadline
  constant in sync.
- Long-running tasks that show **pinned** notifications must use
  [`PinnedProgress`](../view/notification.rs) (or an equivalent drop guard) so
  [`TaskManager::stop`](mod.rs) abort cannot leave UI behind after the stop
  deadline.
