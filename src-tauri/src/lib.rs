//! Gosslan 应用入口（库目标，供 Tauri 加载）。

mod commands;
mod crypto;
mod db;
mod device;
mod gossip_engine;
mod network;
mod protocol;
mod relay;
mod relay_manager;
mod state;
mod storage;
mod transport;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let state = state::AppState::init(app.handle().clone())?;
            state::AppState::spawn_peer_emitter(&state);
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_device_info,
            commands::update_profile,
            commands::list_interfaces,
            commands::get_network_status,
            commands::start_network,
            commands::stop_network,
            commands::get_peers,
            commands::search_nearby_peers,
            commands::focus_window,
            commands::get_topology,
            commands::get_channel_status,
            commands::set_channel_enabled,
            commands::get_cache_info,
            commands::set_cache_policy,
            commands::clean_cache_now,
            commands::get_settings,
            commands::save_settings,
            commands::reset_settings,
            commands::get_friends,
            commands::get_pending_requests,
            commands::send_friend_request,
            commands::respond_friend_request,
            commands::send_message,
            commands::get_messages,
            commands::get_conversations,
            commands::ensure_conversation,
            commands::mark_read,
            commands::create_group,
            commands::distribute_group_key,
            commands::get_groups,
            commands::send_group_message,
            commands::send_file,
            commands::send_file_auto,
            commands::send_file_relay,
            commands::get_transfers,
            commands::set_share_dir,
            commands::get_share_dir,
            commands::request_share_tree,
            commands::download_shared_file,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
