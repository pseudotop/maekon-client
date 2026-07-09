use maekon_core::config::{AiAccessMode, AiProviderType};
use maekon_core::provider_surface_catalog::{
    default_surface_id_for_access_mode, provider_surface_spec, resolved_transport_spec,
    surface_supports_capability, ProviderTransportKind, SurfaceCapabilityKind,
    SurfaceExecutionKind,
};

#[test]
fn full_provider_surface_catalog_resolvers_are_core_owned() {
    let surface = provider_surface_spec("provider_surface.openai.subprocess_cli")
        .expect("core should load provider surface catalog specs");
    assert_eq!(surface.execution_kind, SurfaceExecutionKind::SubprocessCli);

    assert_eq!(
        default_surface_id_for_access_mode(
            AiProviderType::OpenAi,
            AiAccessMode::ProviderSubscriptionCli,
            SurfaceCapabilityKind::Llm,
        )
        .expect("core should resolve access-mode defaults"),
        Some("provider_surface.openai.subprocess_cli")
    );

    let transport = resolved_transport_spec(
        AiProviderType::OpenAi,
        Some("provider_surface.openai.direct_api"),
        ProviderTransportKind::Llm,
    )
    .expect("core should resolve transport specs");
    assert_eq!(transport.url, "https://api.openai.com/v1/responses");

    assert!(surface_supports_capability(
        "provider_surface.openai.subprocess_cli",
        SurfaceCapabilityKind::Ocr,
    )
    .expect("core should resolve declared surface capabilities"));
}
