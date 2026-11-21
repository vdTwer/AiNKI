//! Presets: how decks schedule their cards, and the daily limits.

#[allow(clippy::wildcard_imports, reason = "an area of the service sees the service's shared imports and types")]
use super::*;

/// How a deck with this preset schedules its cards.
///
/// Steps are checked when a preset is saved, so steps that no longer parse
/// mean a damaged row: that is an internal error, not "no steps".
pub(crate) fn sched_config_for(p: &Preset, deck: &Deck) -> Result<SchedConfig> {
    let steps =
        |s: &str| scheduler::parse_steps(s).map_err(|e| anyhow::anyhow!("preset {} has broken steps {s:?}: {e}", p.id));
    Ok(SchedConfig {
        desired_retention: num::f64_f32(p.desired_retention),
        learning_steps: steps(&p.learning_steps)?,
        relearning_steps: steps(&p.relearning_steps)?,
        max_interval: p.max_interval,
        exam_date: deck.exam_date,
    })
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct Preset {
    pub id: i64,
    #[serde(skip)]
    pub user_id: i64,
    pub name: String,
    pub description: String,
    pub desired_retention: f64,
    pub new_per_day: i64,
    pub reviews_per_day: i64,
    pub learning_steps: String,
    pub relearning_steps: String,
    pub max_interval: i64,
    pub created_at: i64,
    /// The built-in template this preset is (`None` for one made by hand).
    pub template: Option<String>,
    /// FSRS parameters of its own, when it has enough reviews; otherwise
    /// the user's apply (see `tuning`).
    #[serde(skip)]
    /// Empty: the account's parameters schedule this preset.
    pub fsrs_params: sqlx::types::Json<Vec<f32>>,
    #[serde(skip)]
    pub fsrs_params_updated_at: Option<i64>,
    /// Seconds to answer a card you have seen before; 0 for no limit.
    pub time_limit: i64,
    /// Type the answer before it is shown.
    pub typed_answers: bool,
}

impl Preset {
    pub fn has_own_params(&self) -> bool {
        !self.fsrs_params.is_empty()
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct PresetInput {
    pub name: Option<String>,
    pub description: Option<String>,
    pub desired_retention: Option<f64>,
    pub new_per_day: Option<i64>,
    pub reviews_per_day: Option<i64>,
    pub learning_steps: Option<String>,
    pub relearning_steps: Option<String>,
    pub max_interval: Option<i64>,
    pub time_limit: Option<i64>,
    pub typed_answers: Option<bool>,
}

impl App {
    pub async fn list_presets(&self, access: &Access) -> Result<Vec<Preset>> {
        Ok(sqlx::query_as("SELECT * FROM presets WHERE user_id = ? ORDER BY id")
            .bind(access.user_id())
            .fetch_all(&self.db)
            .await?)
    }

    pub async fn preset(&self, access: &Access, id: i64) -> Result<Preset> {
        Ok(sqlx::query_as("SELECT * FROM presets WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(access.user_id())
            .fetch_one(&self.db)
            .await?)
    }

    /// Validate a preset and normalize its step lists (e.g. "1m,10m" -> "1m 10m").
    fn validate_preset(p: &mut Preset) -> Result<()> {
        if p.name.trim().is_empty() {
            return Err(AppError::bad("preset name is empty"));
        }
        check_len("preset name", &p.name, MAX_NAME)?;
        check_len("description", &p.description, MAX_DESCRIPTION)?;
        if !(0.70..=0.99).contains(&p.desired_retention) {
            return Err(AppError::bad("desired retention must be between 0.70 and 0.99"));
        }
        if p.new_per_day < 0 || p.reviews_per_day < 0 {
            return Err(AppError::bad("daily limits can't be negative"));
        }
        if p.max_interval < 1 {
            return Err(AppError::bad("maximum interval must be at least 1 day"));
        }
        if !(0..=600).contains(&p.time_limit) {
            return Err(AppError::bad("time limit must be between 0 (none) and 600 seconds"));
        }
        let learning = scheduler::parse_steps(&p.learning_steps).map_err(AppError::BadRequest)?;
        let relearning = scheduler::parse_steps(&p.relearning_steps).map_err(AppError::BadRequest)?;
        p.learning_steps = scheduler::format_steps(&learning);
        p.relearning_steps = scheduler::format_steps(&relearning);
        Ok(())
    }

    fn apply_preset_input(p: &mut Preset, input: PresetInput) {
        if let Some(v) = input.name {
            p.name = v.trim().to_string();
        }
        if let Some(v) = input.description {
            p.description = v;
        }
        if let Some(v) = input.desired_retention {
            p.desired_retention = v;
        }
        if let Some(v) = input.new_per_day {
            p.new_per_day = v;
        }
        if let Some(v) = input.reviews_per_day {
            p.reviews_per_day = v;
        }
        if let Some(v) = input.learning_steps {
            p.learning_steps = v;
        }
        if let Some(v) = input.relearning_steps {
            p.relearning_steps = v;
        }
        if let Some(v) = input.max_interval {
            p.max_interval = v;
        }
        if let Some(v) = input.time_limit {
            p.time_limit = v;
        }
        if let Some(v) = input.typed_answers {
            p.typed_answers = v;
        }
    }

    /// Create a preset, starting from a built-in template (default: standard).
    pub async fn create_preset(&self, access: &Access, template: Option<&str>, input: PresetInput) -> Result<Preset> {
        access.require_write()?;
        access.require_all_decks()?;
        let t = presets::template(template.unwrap_or("standard"))
            .ok_or_else(|| AppError::bad("unknown preset template"))?;
        let mut p = Preset {
            id: 0,
            user_id: access.user_id(),
            name: format!("{} (custom)", t.name),
            description: t.description.to_string(),
            desired_retention: t.desired_retention,
            new_per_day: t.new_per_day,
            reviews_per_day: t.reviews_per_day,
            learning_steps: t.learning_steps.to_string(),
            relearning_steps: t.relearning_steps.to_string(),
            max_interval: t.max_interval,
            created_at: now(),
            template: None,
            fsrs_params: sqlx::types::Json(Vec::new()),
            fsrs_params_updated_at: None,
            time_limit: 0,
            typed_answers: false,
        };
        Self::apply_preset_input(&mut p, input);
        Self::validate_preset(&mut p)?;
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO presets (user_id, name, description, desired_retention, new_per_day,
                reviews_per_day, learning_steps, relearning_steps, max_interval, created_at,
                time_limit, typed_answers)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(p.user_id)
        .bind(&p.name)
        .bind(&p.description)
        .bind(p.desired_retention)
        .bind(p.new_per_day)
        .bind(p.reviews_per_day)
        .bind(&p.learning_steps)
        .bind(&p.relearning_steps)
        .bind(p.max_interval)
        .bind(p.created_at)
        .bind(p.time_limit)
        .bind(p.typed_answers)
        .fetch_one(&self.db)
        .await?;
        self.preset(access, id).await
    }

    pub async fn update_preset(&self, access: &Access, id: i64, input: PresetInput) -> Result<Preset> {
        access.require_write()?;
        access.require_all_decks()?;
        let mut p = self.preset(access, id).await?;
        Self::apply_preset_input(&mut p, input);
        Self::validate_preset(&mut p)?;
        sqlx::query(
            "UPDATE presets SET name = ?, description = ?, desired_retention = ?, new_per_day = ?,
                reviews_per_day = ?, learning_steps = ?, relearning_steps = ?, max_interval = ?,
                time_limit = ?, typed_answers = ?
             WHERE id = ? AND user_id = ?",
        )
        .bind(&p.name)
        .bind(&p.description)
        .bind(p.desired_retention)
        .bind(p.new_per_day)
        .bind(p.reviews_per_day)
        .bind(&p.learning_steps)
        .bind(&p.relearning_steps)
        .bind(p.max_interval)
        .bind(p.time_limit)
        .bind(p.typed_answers)
        .bind(id)
        .bind(access.user_id())
        .execute(&self.db)
        .await?;
        Ok(p)
    }

    pub async fn delete_preset(&self, access: &Access, id: i64) -> Result<()> {
        access.require_write()?;
        access.require_all_decks()?;
        self.preset(access, id).await?;
        let used: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM decks WHERE preset_id = ?").bind(id).fetch_one(&self.db).await?;
        if used > 0 {
            return Err(AppError::bad(format!(
                "{} still {} this preset; switch {} to another preset first",
                crate::format::count(used, "deck"),
                crate::format::agree(used, "uses", "use"),
                crate::format::agree(used, "it", "them")
            )));
        }
        sqlx::query("DELETE FROM presets WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(access.user_id())
            .execute(&self.db)
            .await?;
        Ok(())
    }

    pub(crate) async fn default_preset_id(&self, user_id: i64) -> Result<i64> {
        sqlx::query_scalar("SELECT id FROM presets WHERE user_id = ? ORDER BY id LIMIT 1")
            .bind(user_id)
            .fetch_optional(&self.db)
            .await?
            .ok_or_else(|| AppError::bad("create a preset first"))
    }
}
