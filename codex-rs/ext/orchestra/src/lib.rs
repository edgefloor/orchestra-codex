mod tool;
mod world_state;

pub use tool::AutomationLinearRead;
pub use tool::AutomationLinearReadKind;
pub use tool::AutomationLinearReadStatus;
pub use tool::OrchestraService;

use codex_core::ThreadManager;
use codex_extension_api::ExtensionRegistryBuilder;
use std::sync::Arc;
use std::sync::Weak;

pub fn install(
    registry: &mut ExtensionRegistryBuilder<codex_core::config::Config>,
    thread_manager: Weak<ThreadManager>,
) {
    let service = OrchestraService::new(thread_manager);
    registry.tool_contributor(Arc::new(tool::OrchestraTools::new(service.clone())));
    registry.prompt_contributor(Arc::new(world_state::OrchestraWorldState::new(service)));
}
