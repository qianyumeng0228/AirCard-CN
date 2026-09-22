use std::thread::sleep;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::afc::AfcClient;
use crate::airlift::{
    LINK_PREFIX, RECOVERED_PREFIX, SOURCE_PREFIX, build_books_plist,
    build_streaming_zip_archive, build_streaming_zip_archive_multi, restore_books, snapshot_books,
    stage_streaming_zip,
};
use crate::airtraffic::sync_assets_via_airtraffic;
use crate::device::{ActiveDeviceSession, ConnectionMode};

#[allow(dead_code)]
pub const TARGET_WALLET_ASSETS: &[&str] = &[
    "cardBackgroundCombined@3x.png",
    "cardBackgroundCombined@2x.png",
    "cardBackgroundCombined.pdf",
];

#[allow(dead_code)]
pub const CACHE_FILES: &[&str] = &[
    "FrontFace",
    "PlaceHolder",
    "Preview",
];

#[link(name = "bcrypt")]
unsafe extern "system" {
    fn BCryptGenRandom(
        hAlgorithm: *mut std::ffi::c_void,
        pbBuffer: *mut u8,
        cbBuffer: u32,
        dwFlags: u32,
    ) -> i32;
}

pub fn generate_token() -> String {
    let mut bytes = [0u8; 10];
    unsafe {
        let _ = BCryptGenRandom(
            std::ptr::null_mut(),
            bytes.as_mut_ptr(),
            bytes.len() as u32,
            2, // BCRYPT_USE_SYSTEM_PREFERRED_RNG
        );
    }
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

pub fn write_system_file<L>(
    udid: &str,
    connection_mode: ConnectionMode,
    target_dir: &str,
    leaf_name: &str,
    payload: &[u8],
    mut log: L,
) -> Result<()>
where
    L: FnMut(&str),
{
    let token = generate_token();
    let source = format!("{}{}", SOURCE_PREFIX, token);
    let link_dest = format!("{}{}", LINK_PREFIX, token);
    let recovered = format!("{}{}", RECOVERED_PREFIX, token);

    let link_ident = format!("../../{}/p0/p1/p2/link", source);
    let payload_ident = format!("../../{}/payload", source);
    let target_dest = format!("{}/{}", link_dest, leaf_name);

    let books_identifiers = vec![link_ident.clone(), payload_ident.clone()];
    let assets_to_sync = [
        (link_ident.as_str(), link_dest.as_str()),
        (payload_ident.as_str(), target_dest.as_str()),
    ];

    log(&format!("正在为 {} 连接 AFC...", leaf_name));
    let session = ActiveDeviceSession::open(Some(udid), connection_mode)
        .context("无法打开设备会话进行写入")?;
    log(&format!("已通过 {} 连接。", session.transport.label()));
    let afc = AfcClient::new(&session).context("无法打开 AFC 连接")?;

    let snapshot = snapshot_books(&afc).context("暂存前无法快照 Books 状态")?;

    let archive_data = build_streaming_zip_archive(target_dir, payload)
        .context("无法构建流式 zip 归档")?;

    let books_plist = build_books_plist(&books_identifiers)
        .context("无法构建 Books.plist")?;

    let write_res = (|| -> Result<()> {
        log(&format!("正在通过 MobileInstallation 暂存负载归档（{} 字节）...", archive_data.len()));
        stage_streaming_zip(&session, &source, &archive_data)
            .context("无法暂存流式 zip 通道")?;

        let link_obj = format!("{}/p0/p1/p2/link", source);
        let payload_obj = format!("{}/payload", source);
        if !afc.exists(&source) || !afc.exists(&link_obj) || !afc.exists(&payload_obj) {
            bail!("流式 zip 已完成，但 AFC 上缺少暂存 link/payload 对象");
        }

        afc.make_directory_recursive("Books/Sync")?;
        afc.write_file("Books/Sync/Books.plist", &books_plist)?;
        if !afc.exists("Books/Sync/Books.plist") {
            bail!("无法暂存 Books/Sync/Books.plist");
        }

        log(&format!("正在与 AirTraffic 主机守护进程同步 {}...", leaf_name));
        sync_assets_via_airtraffic(udid, session.transport, &assets_to_sync, &mut log)
            .context("AirTraffic 同步失败")?;

        Ok(())
    })();

    let _ = afc.remove_path(&link_dest);
    let _ = afc.remove_path(&recovered);
    let _ = afc.remove_tree(&source);
    sleep(Duration::from_millis(800));

    let restore_res = restore_books(&afc, &snapshot);

    write_res?;
    restore_res.context("清理期间无法恢复 Books 状态")?;
    log(&format!("写入成功：{}", leaf_name));

    Ok(())
}

pub fn write_system_files_batch<L>(
    udid: &str,
    connection_mode: ConnectionMode,
    target_dir: &str,
    items: &[(&str, &[u8])],
    mut log: L,
) -> Result<()>
where
    L: FnMut(&str),
{
    if items.is_empty() {
        return Ok(());
    }
    if items.len() == 1 {
        return write_system_file(
            udid,
            connection_mode,
            target_dir,
            items[0].0,
            items[0].1,
            log,
        );
    }

    log(&format!("正在为 {} 打包共 {} 个文件的原子批次...", target_dir, items.len()));

    let token = generate_token();
    let source = format!("{}{}", SOURCE_PREFIX, token);
    let link_dest = format!("{}{}", LINK_PREFIX, token);
    let recovered = format!("{}{}", RECOVERED_PREFIX, token);

    let link_ident = format!("../../{}/p0/p1/p2/link", source);
    let mut books_identifiers = Vec::with_capacity(items.len() + 1);
    books_identifiers.push(link_ident.clone());

    let mut assets_to_sync: Vec<(String, String)> = Vec::with_capacity(items.len() + 1);
    assets_to_sync.push((link_ident, link_dest.clone()));

    for (idx, (leaf, _)) in items.iter().enumerate() {
        let payload_ident = format!("../../{}/payload_{}", source, idx);
        let target_dest = format!("{}/{}", link_dest, leaf);
        books_identifiers.push(payload_ident.clone());
        assets_to_sync.push((payload_ident, target_dest));
    }

    log(&format!("正在为共 {} 个素材的批次连接 AFC...", items.len()));
    let session = ActiveDeviceSession::open(Some(udid), connection_mode)
        .context("无法打开设备会话进行写入")?;
    log(&format!("已通过 {} 连接。", session.transport.label()));
    let afc = AfcClient::new(&session).context("无法打开 AFC 连接")?;

    let snapshot = snapshot_books(&afc).context("暂存前无法快照 Books 状态")?;

    let archive_data = build_streaming_zip_archive_multi(target_dir, items)
        .context("无法构建多负载流式 zip 归档")?;

    let books_plist = build_books_plist(&books_identifiers)
        .context("无法为批次构建 Books.plist")?;

    let write_res = (|| -> Result<()> {
        log(&format!("正在通过 MobileInstallation 暂存多负载归档（{} 字节，{} 个文件）...", archive_data.len(), items.len()));
        stage_streaming_zip(&session, &source, &archive_data)
            .context("无法暂存流式 zip 通道")?;

        let link_obj = format!("{}/p0/p1/p2/link", source);
        let payload_obj = format!("{}/payload_0", source);
        let fallback_obj = format!("{}/payload", source);
        if !afc.exists(&source) || !afc.exists(&link_obj) || (!afc.exists(&payload_obj) && !afc.exists(&fallback_obj)) {
            bail!("流式 zip 已完成，但 AFC 上缺少暂存 link/payload 对象");
        }

        afc.make_directory_recursive("Books/Sync")?;
        afc.write_file("Books/Sync/Books.plist", &books_plist)?;
        if !afc.exists("Books/Sync/Books.plist") {
            bail!("无法暂存 Books/Sync/Books.plist");
        }

        log(&format!("正在单个会话中与 AirTraffic 主机守护进程同步批次（{} 项）...", items.len()));
        let assets_refs: Vec<(&str, &str)> = assets_to_sync.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        sync_assets_via_airtraffic(udid, session.transport, &assets_refs, &mut log)
            .context("AirTraffic 批次同步失败")?;

        Ok(())
    })();

    let _ = afc.remove_path(&link_dest);
    let _ = afc.remove_path(&recovered);
    let _ = afc.remove_tree(&source);
    sleep(Duration::from_millis(800));

    let restore_res = restore_books(&afc, &snapshot);

    write_res?;
    restore_res.context("清理期间无法恢复 Books 状态")?;
    log(&format!("共 {} 个文件的批次注入已成功完成！", items.len()));

    Ok(())
}

pub fn flash_wallet_skin<F, L>(
    udid: &str,
    connection_mode: ConnectionMode,
    card_hash: &str,
    skin_png: &[u8],
    skin_pdf: &[u8],
    mut progress: F,
    mut log: L,
) -> Result<()>
where
    F: FnMut(usize, usize, &str),
    L: FnMut(&str),
{
    let pkpass_dir = format!("/var/mobile/Library/Passes/Cards/{}.pkpass", card_hash);

    log(&format!("目标卡片哈希：{}", card_hash));
    log(&format!("卡面负载大小：PNG {} 字节，PDF {} 字节", skin_png.len(), skin_pdf.len()));

    let total_steps = 3;
    progress(1, total_steps, "正在写入卡片图案素材（@3x、@2x、.pdf）...");
    log("[1/3] 正在写入卡片图案素材（@3x.png、@2x.png、cardBackgroundCombined.pdf）...");

    let card_assets: [(&str, &[u8]); 3] = [
        ("cardBackgroundCombined@3x.png", skin_png),
        ("cardBackgroundCombined@2x.png", skin_png),
        ("cardBackgroundCombined.pdf", skin_pdf),
    ];

    if let Err(err) = write_system_files_batch(
        udid,
        connection_mode,
        &pkpass_dir,
        &card_assets,
        &mut log,
    ) {
        log(&format!("提示：批次写入失败（{}），正在尝试逐个写入素材...", err));
        for (asset, data) in &card_assets {
            write_system_file(udid, connection_mode, &pkpass_dir, asset, data, &mut log)
                .context(format!("无法写入卡片素材 {}", asset))?;
        }
    }

    let cache_leaves: [(&str, &[u8]); 3] = [
        ("FrontFace", b"corrupted"),
        ("PlaceHolder", b"corrupted"),
        ("Preview", b"corrupted"),
    ];

    for (c_idx, ext) in [".cache", ".pkcache"].iter().enumerate() {
        let step = 2 + c_idx;
        let cache_dir = format!("/var/mobile/Library/Passes/Cards/{}{}", card_hash, ext);
        progress(step, total_steps, &format!("正在清除 {} 缓存...", ext));
        log(&format!("[{}/{}] 正在使 {} 中的缓存叶失效...", step, total_steps, cache_dir));

        if write_system_files_batch(
            udid,
            connection_mode,
            &cache_dir,
            &cache_leaves,
            &mut log,
        )
        .is_err()
        {
            for (leaf, data) in &cache_leaves {
                let _ = write_system_file(
                    udid,
                    connection_mode,
                    &cache_dir,
                    leaf,
                    data,
                    &mut log,
                );
            }
        }
    }

    progress(total_steps, total_steps, "卡面更新成功！");
    log("卡面写入完成！请在 iPhone 上关闭并重新打开钱包查看。");
    Ok(())
}

pub fn flash_passcode_theme<F, L>(
    udid: &str,
    connection_mode: ConnectionMode,
    items: &[(String, String, Vec<u8>)],
    mut progress: F,
    mut log: L,
) -> Result<()>
where
    F: FnMut(usize, usize, &str),
    L: FnMut(&str),
{
    let total = items.len();
    log(&format!("正在写入密码键盘主题（{} 个素材）...", total));

    let mut dirs_map: std::collections::BTreeMap<String, Vec<(&str, &[u8])>> = std::collections::BTreeMap::new();
    for (tdir, leaf, payload) in items {
        dirs_map.entry(tdir.clone()).or_default().push((leaf.as_str(), payload.as_slice()));
    }

    let total_dirs = dirs_map.len();
    let mut dir_idx = 0;

    for (target_dir, dir_items) in &dirs_map {
        dir_idx += 1;
        let tdir_name = std::path::Path::new(target_dir)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(target_dir);

        progress(
            dir_idx,
            total_dirs,
            &format!("正在写入 {}（原子批次共 {} 个素材）...", tdir_name, dir_items.len()),
        );
        log(&format!("正在将 {} 个素材的批次写入 {}...", dir_items.len(), tdir_name));

        let batch_res = write_system_files_batch(
            udid,
            connection_mode,
            target_dir,
            dir_items,
            &mut log,
        );
        if let Err(err) = batch_res {
            log(&format!("警告：批次写入失败（{}），正在回退为逐个文件写入...", err));
            for (f_idx, (leaf, payload)) in dir_items.iter().enumerate() {
                progress(
                    f_idx + 1,
                    dir_items.len(),
                    &format!("回退 [{}/{}]：正在写入 {}...", f_idx + 1, dir_items.len(), leaf),
                );
                write_system_file(
                    udid,
                    connection_mode,
                    target_dir,
                    leaf,
                    payload,
                    &mut log,
                )
                    .context(format!("无法写入按钮素材 {}", leaf))?;
            }
        }
    }

    progress(total_dirs, total_dirs, "密码键盘主题应用成功！");
    log("密码键盘主题写入成功！锁定或重启 iPhone 即可查看新键盘。");
    Ok(())
}
