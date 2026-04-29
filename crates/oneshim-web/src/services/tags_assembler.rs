use oneshim_api_contracts::tags::TagResponse;
use oneshim_core::models::storage_records::TagRecord;

pub(crate) fn assemble_tag_response(tag: TagRecord) -> TagResponse {
    TagResponse {
        id: tag.id,
        name: tag.name,
        color: tag.color,
        created_at: tag.created_at,
    }
}
