use oneshim_api_contracts::frames::FrameResponse;
use oneshim_core::models::storage_records::FrameRecord;

pub(crate) fn assemble_frame_response(frame: FrameRecord) -> FrameResponse {
    let image_url = frame
        .file_path
        .as_ref()
        .map(|_| format!("/api/frames/{}/image", frame.id));

    FrameResponse {
        id: frame.id,
        timestamp: frame.timestamp,
        trigger_type: frame.trigger_type,
        app_name: frame.app_name,
        window_title: frame.window_title,
        importance: frame.importance,
        resolution: format!("{}x{}", frame.resolution_w, frame.resolution_h),
        file_path: frame.file_path,
        ocr_text: frame.ocr_text,
        image_url,
        tag_ids: Vec::new(),
    }
}
