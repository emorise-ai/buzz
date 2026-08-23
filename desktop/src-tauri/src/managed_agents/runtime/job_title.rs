use crate::managed_agents::{AgentDefinition, ManagedAgentRecord};

pub(super) fn resolve_job_title(
    record: &ManagedAgentRecord,
    personas: &[AgentDefinition],
) -> Option<String> {
    if let Some(persona_id) = record.persona_id.as_deref() {
        if let Some(persona) = personas.iter().find(|persona| persona.id == persona_id) {
            return persona.job_title.clone();
        }
    }
    record.job_title.clone()
}

#[cfg(test)]
mod tests {
    use super::resolve_job_title;
    use crate::managed_agents::{runtime::test_fixtures::fixture, AgentDefinition, RespondTo};

    #[test]
    fn cleared_persona_title_does_not_fall_back_to_materialized_record_title() {
        let mut record = fixture(RespondTo::OwnerOnly, vec![], None);
        record.persona_id = Some("persona".into());
        record.job_title = Some("Old Title".into());
        let persona = AgentDefinition {
            id: "persona".into(),
            display_name: "Persona".into(),
            job_title: None,
            avatar_url: None,
            system_prompt: String::new(),
            runtime: None,
            model: None,
            provider: None,
            name_pool: Vec::new(),
            is_builtin: false,
            is_active: true,
            shared: false,
            source_team: None,
            source_team_persona_slug: None,
            catalog_source: None,
            env_vars: Default::default(),
            respond_to: None,
            respond_to_allowlist: Vec::new(),
            parallelism: None,
            created_at: String::new(),
            updated_at: String::new(),
        };

        assert_eq!(resolve_job_title(&record, &[persona]), None);
    }
}
