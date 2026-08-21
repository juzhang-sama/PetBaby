use rusqlite::Connection;

const LEGACY_REPAIR_TARGET_VERSION: i64 = 6;
const UNIFIED_CREATION_SCHEMA_VERSION: usize = 6;
const PET_PROFILE_SCHEMA_VERSION: usize = 8;

pub const MIGRATIONS: &[&str] = &[
    // v1: pets, variants and state tables
    r#"
    CREATE TABLE pets (
      pet_id TEXT PRIMARY KEY,
      schema_version INTEGER NOT NULL,
      species TEXT NOT NULL,
      identity_mode TEXT NOT NULL,
      created_at TEXT NOT NULL,
      updated_at TEXT NOT NULL
    );
    CREATE TABLE variants (
      variant_id TEXT PRIMARY KEY,
      pet_id TEXT NOT NULL REFERENCES pets(pet_id) ON DELETE CASCADE,
      style_id TEXT NOT NULL,
      manifest_path TEXT NOT NULL,
      created_at TEXT NOT NULL
    );
    CREATE TABLE state (
      key TEXT PRIMARY KEY,
      value TEXT NOT NULL
    );
    "#,
    // v2: creation domain (profiles, generation jobs, appearance variants)
    r#"
    CREATE TABLE identity_profiles (
      profile_id TEXT PRIMARY KEY,
      pet_id TEXT NOT NULL REFERENCES pets(pet_id) ON DELETE CASCADE,
      schema_version INTEGER NOT NULL,
      species TEXT NOT NULL,
      identity_mode TEXT NOT NULL,
      locked_traits TEXT NOT NULL,
      ref_asset_id TEXT,
      created_at TEXT NOT NULL
    );
    CREATE TABLE generation_jobs (
      job_id TEXT PRIMARY KEY,
      pet_id TEXT NOT NULL REFERENCES pets(pet_id) ON DELETE CASCADE,
      prompt TEXT NOT NULL,
      ref_sha256 TEXT NOT NULL,
      task_id TEXT,
      status TEXT NOT NULL,
      result_url TEXT,
      error TEXT,
      created_at TEXT NOT NULL
    );
    CREATE TABLE appearance_variants (
      variant_id TEXT PRIMARY KEY,
      pet_id TEXT NOT NULL REFERENCES pets(pet_id) ON DELETE CASCADE,
      job_id TEXT REFERENCES generation_jobs(job_id),
      image_path TEXT NOT NULL,
      cutout_path TEXT,
      quality TEXT NOT NULL,
      accepted INTEGER NOT NULL DEFAULT 0,
      created_at TEXT NOT NULL
    );
    "#,
    // v3: unified creation sessions, recipes and pet creation metadata
    r#"
    ALTER TABLE pets ADD COLUMN display_name TEXT;
    ALTER TABLE pets ADD COLUMN creation_method TEXT NOT NULL DEFAULT 'upload'
      CHECK(creation_method IN ('upload','composer','adoption'));
    ALTER TABLE pets ADD COLUMN source_template_id TEXT;
    ALTER TABLE pets ADD COLUMN source_template_version INTEGER;
    ALTER TABLE pets ADD COLUMN lifecycle TEXT NOT NULL DEFAULT 'ready';
    ALTER TABLE pets ADD COLUMN completed_at TEXT;

    UPDATE pets SET display_name = '我的猫咪', completed_at = created_at;
    UPDATE pets SET creation_method = CASE identity_mode
      WHEN 'guided' THEN 'composer'
      WHEN 'adopted' THEN 'adoption'
      ELSE 'upload'
    END;

    CREATE TABLE creation_sessions (
      session_id TEXT PRIMARY KEY,
      pet_id TEXT NOT NULL UNIQUE REFERENCES pets(pet_id) ON DELETE CASCADE,
      method TEXT NOT NULL CHECK(method IN ('upload','composer','adoption')),
      status TEXT NOT NULL CHECK(status IN
        ('draft','candidateReady','finalizing','retryableFailure','completed','abandoned')),
      last_stable_status TEXT NOT NULL,
      current_step TEXT NOT NULL,
      schema_version INTEGER NOT NULL,
      error TEXT,
      created_at TEXT NOT NULL,
      updated_at TEXT NOT NULL,
      completed_at TEXT
    );

    CREATE TABLE composer_recipes (
      session_id TEXT PRIMARY KEY REFERENCES creation_sessions(session_id) ON DELETE CASCADE,
      recipe_version INTEGER NOT NULL,
      pack_id TEXT NOT NULL,
      pack_version INTEGER NOT NULL,
      layer_contract_version INTEGER NOT NULL,
      body_id TEXT NOT NULL,
      ears_id TEXT NOT NULL,
      eyes_id TEXT NOT NULL,
      muzzle_id TEXT NOT NULL,
      tail_id TEXT NOT NULL,
      color_id TEXT NOT NULL,
      pattern_id TEXT NOT NULL,
      updated_at TEXT NOT NULL
    );

    CREATE TABLE creation_session_tombstones (
      session_id TEXT PRIMARY KEY,
      pet_id TEXT NOT NULL,
      method TEXT NOT NULL,
      abandoned_at TEXT NOT NULL
    );

    ALTER TABLE generation_jobs
      ADD COLUMN session_id TEXT REFERENCES creation_sessions(session_id);
    ALTER TABLE appearance_variants
      ADD COLUMN session_id TEXT REFERENCES creation_sessions(session_id);
    ALTER TABLE appearance_variants ADD COLUMN motion_profile_path TEXT;

    CREATE INDEX generation_jobs_session_idx ON generation_jobs(session_id);
    CREATE INDEX appearance_variants_session_idx ON appearance_variants(session_id);
    CREATE UNIQUE INDEX pets_unique_adoption_source
      ON pets(source_template_id)
      WHERE source_template_id IS NOT NULL
        AND creation_method = 'adoption'
        AND lifecycle != 'abandoned';

    CREATE TRIGGER pets_validate_source_template_insert
    BEFORE INSERT ON pets
    WHEN NOT (
      (NEW.source_template_id IS NULL AND NEW.source_template_version IS NULL)
      OR (
        NEW.source_template_id IS NOT NULL
        AND NEW.source_template_version IS NOT NULL
        AND NEW.creation_method = 'adoption'
      )
    )
    BEGIN
      SELECT RAISE(ABORT, 'invalid pet source template');
    END;

    CREATE TRIGGER pets_validate_source_template_update
    BEFORE UPDATE OF source_template_id, source_template_version, creation_method ON pets
    WHEN NOT (
      (NEW.source_template_id IS NULL AND NEW.source_template_version IS NULL)
      OR (
        NEW.source_template_id IS NOT NULL
        AND NEW.source_template_version IS NOT NULL
        AND NEW.creation_method = 'adoption'
      )
    )
    BEGIN
      SELECT RAISE(ABORT, 'invalid pet source template');
    END;

    INSERT INTO creation_sessions (
      session_id, pet_id, method, status, last_stable_status, current_step,
      schema_version, created_at, updated_at, completed_at
    )
    SELECT
      'session-migrated-' || p.pet_id,
      p.pet_id,
      p.creation_method,
      CASE
        WHEN EXISTS (
          SELECT 1 FROM appearance_variants av
          WHERE av.pet_id = p.pet_id AND av.accepted = 1
        ) THEN 'completed'
        WHEN EXISTS (
          SELECT 1 FROM appearance_variants av WHERE av.pet_id = p.pet_id
        ) THEN 'candidateReady'
        ELSE 'draft'
      END,
      CASE
        WHEN EXISTS (
          SELECT 1 FROM appearance_variants av
          WHERE av.pet_id = p.pet_id AND av.accepted = 1
        ) THEN 'completed'
        WHEN EXISTS (
          SELECT 1 FROM appearance_variants av WHERE av.pet_id = p.pet_id
        ) THEN 'candidateReady'
        ELSE 'draft'
      END,
      CASE
        WHEN EXISTS (
          SELECT 1 FROM appearance_variants av
          WHERE av.pet_id = p.pet_id AND av.accepted = 1
        ) THEN 'completed'
        WHEN EXISTS (
          SELECT 1 FROM appearance_variants av WHERE av.pet_id = p.pet_id
        ) THEN 'review'
        ELSE p.creation_method
      END,
      1,
      p.created_at,
      p.updated_at,
      CASE
        WHEN EXISTS (
          SELECT 1 FROM appearance_variants av
          WHERE av.pet_id = p.pet_id AND av.accepted = 1
        ) THEN p.created_at
        ELSE NULL
      END
    FROM pets p;

    WITH ranked AS (
      SELECT cs.session_id,
             ROW_NUMBER() OVER (ORDER BY p.created_at DESC, p.rowid DESC) AS draft_rank
      FROM creation_sessions cs
      JOIN pets p ON p.pet_id = cs.pet_id
      WHERE cs.method IN ('upload','composer')
        AND cs.status NOT IN ('completed','abandoned')
    )
    UPDATE creation_sessions
    SET status = 'abandoned',
        last_stable_status = 'abandoned',
        current_step = 'abandoned'
    WHERE session_id IN (
      SELECT session_id FROM ranked WHERE draft_rank > 1
    );

    UPDATE pets
    SET lifecycle = 'abandoned'
    WHERE pet_id IN (
      SELECT pet_id FROM creation_sessions WHERE status = 'abandoned'
    );

    UPDATE generation_jobs
    SET session_id = (
      SELECT cs.session_id FROM creation_sessions cs
      WHERE cs.pet_id = generation_jobs.pet_id
    );
    UPDATE appearance_variants
    SET session_id = (
      SELECT cs.session_id FROM creation_sessions cs
      WHERE cs.pet_id = appearance_variants.pet_id
    );
    UPDATE appearance_variants
    SET motion_profile_path = replace(cutout_path, 'cutout.png', 'motion-profile.json')
    WHERE cutout_path IS NOT NULL
      AND cutout_path LIKE '%cutout.png'
      AND pet_id IN (
        SELECT pet_id FROM pets WHERE creation_method = 'upload'
      );

    CREATE UNIQUE INDEX creation_one_long_draft
      ON creation_sessions ((1))
      WHERE method IN ('upload','composer')
        AND status NOT IN ('completed','abandoned');
    "#,
    // v4: durable upload source owned by a creation session
    r#"
    CREATE TABLE creation_upload_sources (
      session_id TEXT PRIMARY KEY
        REFERENCES creation_sessions(session_id) ON DELETE CASCADE,
      normalized_png BLOB NOT NULL,
      sha256 TEXT NOT NULL CHECK(length(sha256) = 64),
      mime_type TEXT NOT NULL CHECK(mime_type = 'image/png'),
      byte_size INTEGER NOT NULL CHECK(byte_size > 0 AND byte_size <= 10485760),
      created_at TEXT NOT NULL,
      CHECK(length(normalized_png) = byte_size)
    );
    "#,
    // v5: immutable template provenance owned by an adoption session
    r#"
    CREATE TABLE creation_adoption_provenance (
      session_id TEXT PRIMARY KEY
        REFERENCES creation_sessions(session_id) ON DELETE CASCADE,
      source_template_id TEXT NOT NULL,
      source_template_version INTEGER NOT NULL CHECK(source_template_version > 0),
      runtime_schema_version INTEGER NOT NULL CHECK(runtime_schema_version > 0),
      body_sha256 TEXT NOT NULL
        CHECK(length(body_sha256) = 64 AND body_sha256 NOT GLOB '*[^0-9a-f]*'),
      motion_profile_sha256 TEXT NOT NULL
        CHECK(length(motion_profile_sha256) = 64
              AND motion_profile_sha256 NOT GLOB '*[^0-9a-f]*'),
      created_at TEXT NOT NULL
    );
    "#,
    // v6: validate the unified creation schema after the legacy version collision repair
    r#"
    SELECT pet_id, schema_version, species, identity_mode, display_name,
           creation_method, source_template_id, source_template_version,
           lifecycle, created_at, updated_at, completed_at
    FROM pets LIMIT 0;
    SELECT variant_id, pet_id, style_id, manifest_path, created_at
    FROM variants LIMIT 0;
    SELECT key, value FROM state LIMIT 0;
    SELECT profile_id, pet_id, schema_version, species, identity_mode,
           locked_traits, ref_asset_id, created_at
    FROM identity_profiles LIMIT 0;
    SELECT job_id, pet_id, prompt, ref_sha256, task_id, status, result_url,
           error, created_at, session_id
    FROM generation_jobs LIMIT 0;
    SELECT variant_id, pet_id, job_id, image_path, cutout_path, quality,
           accepted, created_at, session_id, motion_profile_path
    FROM appearance_variants LIMIT 0;
    SELECT session_id, pet_id, method, status, last_stable_status, current_step,
           schema_version, error, created_at, updated_at, completed_at
    FROM creation_sessions LIMIT 0;
    SELECT session_id, recipe_version, pack_id, pack_version, layer_contract_version,
           body_id, ears_id, eyes_id, muzzle_id, tail_id, color_id, pattern_id, updated_at
    FROM composer_recipes LIMIT 0;
    SELECT session_id, pet_id, method, abandoned_at
    FROM creation_session_tombstones LIMIT 0;
    SELECT session_id, normalized_png, sha256, mime_type, byte_size, created_at
    FROM creation_upload_sources LIMIT 0;
    SELECT session_id, source_template_id, source_template_version,
           runtime_schema_version, body_sha256, motion_profile_sha256, created_at
    FROM creation_adoption_provenance LIMIT 0;
    "#,
    // v7: versioned diagnostic report for generated candidates
    r#"
    ALTER TABLE appearance_variants ADD COLUMN quality_report_json TEXT;
    "#,
    // v8: editable pet profile fields, separate from immutable creation identity
    r#"
    CREATE TABLE pet_profiles (
      pet_id TEXT PRIMARY KEY REFERENCES pets(pet_id) ON DELETE CASCADE,
      schema_version INTEGER NOT NULL DEFAULT 1 CHECK(schema_version = 1),
      gender_code TEXT CHECK(gender_code IN ('male','female') OR gender_code IS NULL),
      birth_date TEXT CHECK(
        birth_date IS NULL OR (
          typeof(birth_date) = 'text'
          AND length(birth_date) = 10
          AND birth_date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]'
          AND CAST(substr(birth_date, 1, 4) AS INTEGER) BETWEEN 1 AND 9999
          AND CAST(substr(birth_date, 6, 2) AS INTEGER) BETWEEN 1 AND 12
          AND CAST(substr(birth_date, 9, 2) AS INTEGER) BETWEEN 1 AND CASE
            CAST(substr(birth_date, 6, 2) AS INTEGER)
            WHEN 2 THEN CASE
              WHEN CAST(substr(birth_date, 1, 4) AS INTEGER) % 400 = 0
                OR (
                  CAST(substr(birth_date, 1, 4) AS INTEGER) % 4 = 0
                  AND CAST(substr(birth_date, 1, 4) AS INTEGER) % 100 != 0
                ) THEN 29
              ELSE 28
            END
            WHEN 4 THEN 30
            WHEN 6 THEN 30
            WHEN 9 THEN 30
            WHEN 11 THEN 30
            ELSE 31
          END
        )
      ),
      updated_at TEXT NOT NULL
    );
    INSERT INTO pet_profiles(pet_id, schema_version, gender_code, birth_date, updated_at)
    SELECT pet_id, 1, NULL, NULL, updated_at
    FROM pets
    WHERE lifecycle='ready' AND completed_at IS NOT NULL;
    "#,
    // v9: durable photo-avatar generation sessions, remote attempts and provider outputs
    r#"
    CREATE TABLE photo_avatar_consents (
      consent_version TEXT PRIMARY KEY CHECK(consent_version='photo-avatar-third-party-ai-v1'),
      accepted_at TEXT NOT NULL
    );
    CREATE TABLE photo_avatar_sources (
      session_id TEXT NOT NULL REFERENCES creation_sessions(session_id) ON DELETE CASCADE,
      source_id TEXT NOT NULL,
      ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
      normalized_png BLOB NOT NULL,
      sha256 TEXT NOT NULL CHECK(length(sha256)=64 AND sha256 NOT GLOB '*[^0-9a-f]*'),
      width INTEGER NOT NULL CHECK(width BETWEEN 256 AND 4096),
      height INTEGER NOT NULL CHECK(height BETWEEN 256 AND 4096),
      byte_size INTEGER NOT NULL CHECK(byte_size > 0 AND byte_size <= 10485760),
      created_at TEXT NOT NULL,
      PRIMARY KEY(session_id, source_id),
      UNIQUE(session_id, ordinal),
      CHECK(length(normalized_png)=byte_size)
    );
    CREATE TABLE photo_avatar_runs (
      session_id TEXT PRIMARY KEY REFERENCES creation_sessions(session_id) ON DELETE CASCADE,
      revision INTEGER NOT NULL CHECK(revision >= 1),
      step TEXT NOT NULL CHECK(step IN ('collecting','analyzeIdentity','completeAppearance','renderTextureAtlas','buildV5','runtimeCheckPending','previewReady','cleanupPending','completed','failed','cancelled')),
      provider_session_id TEXT,
      provider_job_id TEXT,
      generation_token TEXT NOT NULL,
      modification_instruction TEXT,
      locked_trait_keys_json TEXT,
      error_code TEXT,
      error_message TEXT,
      updated_at TEXT NOT NULL
    );
    CREATE TABLE photo_avatar_step_attempts (
      session_id TEXT NOT NULL REFERENCES photo_avatar_runs(session_id) ON DELETE CASCADE,
      revision INTEGER NOT NULL,
      step TEXT NOT NULL CHECK(step IN ('analyzeIdentity','completeAppearance','renderTextureAtlas')),
      attempt_no INTEGER NOT NULL CHECK(attempt_no BETWEEN 1 AND 3),
      provider_job_id TEXT,
      status TEXT NOT NULL CHECK(status IN ('submitted','running','succeeded','failed','cancelled','superseded')),
      retryable INTEGER NOT NULL CHECK(retryable IN (0,1)),
      error_code TEXT,
      started_at TEXT NOT NULL,
      finished_at TEXT,
      PRIMARY KEY(session_id, revision, step, attempt_no)
    );
    CREATE TABLE photo_avatar_profiles (
      session_id TEXT NOT NULL REFERENCES photo_avatar_runs(session_id) ON DELETE CASCADE,
      revision INTEGER NOT NULL,
      schema_version INTEGER NOT NULL CHECK(schema_version=1),
      body_module_id TEXT NOT NULL CHECK(body_module_id IN ('body-slender-v1','body-balanced-v1','body-rounded-v1')),
      profile_json TEXT NOT NULL,
      profile_sha256 TEXT NOT NULL CHECK(length(profile_sha256)=64),
      created_at TEXT NOT NULL,
      PRIMARY KEY(session_id, revision)
    );
    CREATE TABLE photo_avatar_artifacts (
      session_id TEXT NOT NULL REFERENCES photo_avatar_runs(session_id) ON DELETE CASCADE,
      revision INTEGER NOT NULL,
      kind TEXT NOT NULL CHECK(kind IN ('textureAtlas','previewPackage')),
      relative_path TEXT NOT NULL,
      sha256 TEXT NOT NULL CHECK(length(sha256)=64),
      local_path TEXT,
      created_at TEXT NOT NULL,
      PRIMARY KEY(session_id, revision, kind)
    );
    CREATE INDEX photo_avatar_attempt_lookup
      ON photo_avatar_step_attempts(session_id, revision, step, attempt_no DESC);
    "#,
    // v10: explicit lk888 v2 disclosure and three-domain cleanup audit
    r#"
    ALTER TABLE photo_avatar_consents RENAME TO photo_avatar_consents_v9;
    CREATE TABLE photo_avatar_consents (
      consent_version TEXT PRIMARY KEY CHECK(consent_version IN (
        'photo-avatar-third-party-ai-v1',
        'photo-avatar-third-party-ai-lk888-no-delete-v2'
      )),
      provider_id TEXT,
      disclosure_sha256 TEXT,
      accepted_at TEXT NOT NULL,
      CHECK(
        (consent_version='photo-avatar-third-party-ai-v1'
         AND provider_id IS NULL AND disclosure_sha256 IS NULL)
        OR
        (consent_version='photo-avatar-third-party-ai-lk888-no-delete-v2'
         AND provider_id='lk888'
         AND disclosure_sha256='fa6ad319cea369bb51349b9b16d11544ecab71ba0bbb027c32b624f72c86a3be')
      )
    );
    INSERT INTO photo_avatar_consents(consent_version, provider_id, disclosure_sha256, accepted_at)
    SELECT consent_version, NULL, NULL, accepted_at FROM photo_avatar_consents_v9;
    DROP TABLE photo_avatar_consents_v9;

    CREATE TABLE photo_avatar_cleanup_audit (
      session_id TEXT NOT NULL,
      revision INTEGER NOT NULL CHECK(revision >= 1),
      local_cleanup TEXT NOT NULL CHECK(local_cleanup IN ('deleted','pending')),
      backend_cleanup TEXT NOT NULL CHECK(backend_cleanup IN ('deleted','pending')),
      upstream_cleanup TEXT NOT NULL CHECK(upstream_cleanup='unsupported'),
      provider_id TEXT NOT NULL CHECK(provider_id='lk888'),
      updated_at TEXT NOT NULL,
      PRIMARY KEY(session_id, revision)
    );
    "#,
    // v11: bind canonical texture audit to the immutable provider artifact
    r#"
    ALTER TABLE photo_avatar_artifacts ADD COLUMN audit_json TEXT;
    "#,
    // v12: keep frozen Live2D history while adding an unambiguous pixel route
    r#"
    ALTER TABLE photo_avatar_runs ADD COLUMN route TEXT NOT NULL DEFAULT 'live2d-v5'
      CHECK(route IN ('live2d-v5','pixel-v1'));
    ALTER TABLE photo_avatar_runs RENAME TO photo_avatar_runs_v11;
    CREATE TABLE photo_avatar_runs (
      session_id TEXT PRIMARY KEY REFERENCES creation_sessions(session_id) ON DELETE CASCADE,
      revision INTEGER NOT NULL CHECK(revision >= 1),
      route TEXT NOT NULL DEFAULT 'live2d-v5' CHECK(route IN ('live2d-v5','pixel-v1')),
      step TEXT NOT NULL CHECK(step IN (
        'collecting','analyzeIdentity','completeAppearance','renderTextureAtlas','buildV5',
        'generatePixelAvatar','qualityCheckPending','runtimeCheckPending','previewReady',
        'cleanupPending','completed','failed','cancelled'
      )),
      provider_session_id TEXT,
      provider_job_id TEXT,
      generation_token TEXT NOT NULL,
      modification_instruction TEXT,
      locked_trait_keys_json TEXT,
      error_code TEXT,
      error_message TEXT,
      updated_at TEXT NOT NULL
    );
    INSERT INTO photo_avatar_runs(
      session_id, revision, route, step, provider_session_id, provider_job_id,
      generation_token, modification_instruction, locked_trait_keys_json,
      error_code, error_message, updated_at
    )
    SELECT session_id, revision, route, step, provider_session_id, provider_job_id,
           generation_token, modification_instruction, locked_trait_keys_json,
           error_code, error_message, updated_at
    FROM photo_avatar_runs_v11;

    ALTER TABLE photo_avatar_step_attempts RENAME TO photo_avatar_step_attempts_v11;
    CREATE TABLE photo_avatar_step_attempts (
      session_id TEXT NOT NULL REFERENCES photo_avatar_runs(session_id) ON DELETE CASCADE,
      revision INTEGER NOT NULL,
      route TEXT NOT NULL DEFAULT 'live2d-v5' CHECK(route IN ('live2d-v5','pixel-v1')),
      step TEXT NOT NULL CHECK(step IN ('analyzeIdentity','completeAppearance','renderTextureAtlas','generatePixelAvatar')),
      attempt_no INTEGER NOT NULL CHECK(attempt_no BETWEEN 1 AND 3),
      provider_job_id TEXT,
      status TEXT NOT NULL CHECK(status IN ('submitted','running','succeeded','failed','cancelled','superseded')),
      retryable INTEGER NOT NULL CHECK(retryable IN (0,1)),
      error_code TEXT,
      started_at TEXT NOT NULL,
      finished_at TEXT,
      PRIMARY KEY(session_id, revision, route, step, attempt_no)
    );
    INSERT INTO photo_avatar_step_attempts(
      session_id, revision, route, step, attempt_no, provider_job_id, status,
      retryable, error_code, started_at, finished_at
    )
    SELECT session_id, revision, 'live2d-v5', step, attempt_no, provider_job_id, status,
           retryable, error_code, started_at, finished_at
    FROM photo_avatar_step_attempts_v11;
    DROP TABLE photo_avatar_step_attempts_v11;
    CREATE INDEX photo_avatar_attempt_lookup
      ON photo_avatar_step_attempts(session_id, revision, route, step, attempt_no DESC);

    ALTER TABLE photo_avatar_profiles RENAME TO photo_avatar_profiles_v11;
    CREATE TABLE photo_avatar_profiles (
      session_id TEXT NOT NULL REFERENCES photo_avatar_runs(session_id) ON DELETE CASCADE,
      revision INTEGER NOT NULL,
      route TEXT NOT NULL DEFAULT 'live2d-v5' CHECK(route IN ('live2d-v5','pixel-v1')),
      profile_kind TEXT NOT NULL DEFAULT 'live2d-v5' CHECK(profile_kind IN ('live2d-v5','pixel-v1')),
      schema_version INTEGER NOT NULL CHECK(schema_version=1),
      body_module_id TEXT,
      profile_json TEXT NOT NULL,
      profile_sha256 TEXT NOT NULL CHECK(length(profile_sha256)=64),
      created_at TEXT NOT NULL,
      PRIMARY KEY(session_id, revision, route)
    );
    INSERT INTO photo_avatar_profiles(
      session_id, revision, route, profile_kind, schema_version, body_module_id,
      profile_json, profile_sha256, created_at
    )
    SELECT session_id, revision, 'live2d-v5', 'live2d-v5', schema_version, body_module_id,
           profile_json, profile_sha256, created_at
    FROM photo_avatar_profiles_v11;
    DROP TABLE photo_avatar_profiles_v11;

    ALTER TABLE photo_avatar_artifacts RENAME TO photo_avatar_artifacts_v11;
    CREATE TABLE photo_avatar_artifacts (
      session_id TEXT NOT NULL REFERENCES photo_avatar_runs(session_id) ON DELETE CASCADE,
      revision INTEGER NOT NULL,
      route TEXT NOT NULL DEFAULT 'live2d-v5' CHECK(route IN ('live2d-v5','pixel-v1')),
      kind TEXT NOT NULL CHECK(kind IN ('textureAtlas','previewPackage','pixelAvatar')),
      relative_path TEXT NOT NULL,
      sha256 TEXT NOT NULL CHECK(length(sha256)=64),
      local_path TEXT,
      audit_json TEXT,
      created_at TEXT NOT NULL,
      PRIMARY KEY(session_id, revision, route, kind)
    );
    INSERT INTO photo_avatar_artifacts(
      session_id, revision, route, kind, relative_path, sha256, local_path, audit_json, created_at
    )
    SELECT session_id, revision, 'live2d-v5', kind, relative_path, sha256, local_path, audit_json, created_at
    FROM photo_avatar_artifacts_v11;
    DROP TABLE photo_avatar_artifacts_v11;
    DROP TABLE photo_avatar_runs_v11;
    "#,
    r#"
    ALTER TABLE photo_avatar_runs ADD COLUMN style_profile_id TEXT NOT NULL
      DEFAULT 'pixel-style-v1'
      CHECK(style_profile_id IN ('live2d-v5','pixel-style-v1','pixel-style-v2-animation-ready'));
    UPDATE photo_avatar_runs
    SET style_profile_id='live2d-v5'
    WHERE route='live2d-v5';
    "#,
];

fn table_exists(db: &Connection, table: &str) -> Result<bool, String> {
    db.query_row(
        "SELECT EXISTS(
           SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1
         )",
        [table],
        |row| row.get(0),
    )
    .map_err(|error| error.to_string())
}

fn has_columns(db: &Connection, table: &str, required: &[&str]) -> Result<bool, String> {
    for column in required {
        let exists: bool = db
            .query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM pragma_table_info(?1) WHERE name=?2
                 )",
                [table, column],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if !exists {
            return Ok(false);
        }
    }
    Ok(true)
}

fn normalized_sql(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn require_columns(db: &Connection, table: &str, required: &[&str]) -> Result<(), String> {
    if !table_exists(db, table)? {
        return Err(format!("schema validation failed: missing table {table}"));
    }
    for column in required {
        if !has_columns(db, table, &[*column])? {
            return Err(format!(
                "schema validation failed: missing {table}.{column}"
            ));
        }
    }
    Ok(())
}

fn schema_sql(db: &Connection, object_type: &str, name: &str) -> Result<String, String> {
    let sql: Option<String> = db
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type=?1 AND name=?2",
            [object_type, name],
            |row| row.get(0),
        )
        .map_err(|_| format!("schema validation failed: missing {object_type} {name}"))?;
    sql.ok_or_else(|| format!("schema validation failed: empty SQL for {name}"))
}

fn require_reference_columns(
    db: &Connection,
    reference: &Connection,
    table: &str,
    columns: &[&str],
) -> Result<(), String> {
    for column in columns {
        let read_contract = |connection: &Connection| {
            connection.query_row(
                "SELECT type, \"notnull\", dflt_value, pk
                 FROM pragma_table_info(?1) WHERE name=?2",
                [table, column],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
        };
        let expected = read_contract(reference).map_err(|error| error.to_string())?;
        let actual = read_contract(db)
            .map_err(|_| format!("schema validation failed: missing {table}.{column}"))?;
        if actual != expected {
            return Err(format!(
                "schema validation failed: {table}.{column} has the wrong column contract"
            ));
        }
    }
    Ok(())
}

fn foreign_key_contracts(
    db: &Connection,
    table: &str,
) -> Result<Vec<(String, String, Option<String>, String, String, String)>, String> {
    let mut statement = db
        .prepare(
            "SELECT \"table\", \"from\", \"to\", on_update, on_delete, match
             FROM pragma_foreign_key_list(?1) ORDER BY id, seq",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([table], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        })
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

fn require_reference_foreign_keys(
    db: &Connection,
    reference: &Connection,
    table: &str,
) -> Result<(), String> {
    let expected = foreign_key_contracts(reference, table)?;
    let actual = foreign_key_contracts(db, table)?;
    for contract in expected {
        if !actual.contains(&contract) {
            return Err(format!(
                "schema validation failed: {table} has the wrong foreign key contract"
            ));
        }
    }
    Ok(())
}

fn require_reference_object(
    db: &Connection,
    reference: &Connection,
    object_type: &str,
    name: &str,
) -> Result<(), String> {
    let expected = normalized_sql(&schema_sql(reference, object_type, name)?);
    let actual = normalized_sql(&schema_sql(db, object_type, name)?);
    if actual != expected {
        return Err(format!(
            "schema validation failed: {object_type} {name} has the wrong definition"
        ));
    }
    Ok(())
}

fn validate_pet_creation_method_contract(db: &Connection) -> Result<(), String> {
    let pets_sql = schema_sql(db, "table", "pets")?;
    let required_check =
        normalized_sql("CHECK(creation_method IN ('upload','composer','adoption'))");
    if !normalized_sql(&pets_sql).contains(&required_check) {
        return Err(
            "schema validation failed: pets.creation_method CHECK has the wrong definition".into(),
        );
    }
    let invalid_rows: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM pets
             WHERE creation_method IS NULL
                OR creation_method NOT IN ('upload','composer','adoption')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if invalid_rows != 0 {
        return Err(format!(
            "schema validation failed: pets contains {invalid_rows} row(s) with invalid creation_method data"
        ));
    }
    let probe = Connection::open_in_memory().map_err(|error| error.to_string())?;
    probe
        .execute_batch(&pets_sql)
        .map_err(|error| format!("schema validation failed: invalid pets SQL: {error}"))?;
    probe
        .execute(
            "INSERT INTO pets
             (pet_id, schema_version, species, identity_mode, creation_method,
              lifecycle, created_at, updated_at)
             VALUES ('schema-probe', 1, 'cat', 'realpet', 'upload', 'draft', '1', '1')",
            [],
        )
        .map_err(|error| {
            format!(
                "schema validation failed: pets.creation_method CHECK rejects a valid value: {error}"
            )
        })?;
    for method in ["composer", "adoption", "upload"] {
        probe
            .execute(
                "UPDATE pets SET creation_method=?1 WHERE pet_id='schema-probe'",
                [method],
            )
            .map_err(|error| {
                format!(
                    "schema validation failed: pets.creation_method CHECK rejects {method}: {error}"
                )
            })?;
    }
    if probe
        .execute(
            "UPDATE pets SET creation_method='invalid' WHERE pet_id='schema-probe'",
            [],
        )
        .is_ok()
    {
        return Err(
            "schema validation failed: pets.creation_method CHECK accepts an invalid value".into(),
        );
    }
    Ok(())
}

fn validate_unified_schema(db: &Connection) -> Result<(), String> {
    let reference = Connection::open_in_memory().map_err(|error| error.to_string())?;
    for migration in &MIGRATIONS[..5] {
        reference
            .execute_batch(migration)
            .map_err(|error| format!("failed to build reference schema: {error}"))?;
    }

    for (table, columns) in [
        (
            "pets",
            &[
                "pet_id",
                "schema_version",
                "species",
                "identity_mode",
                "display_name",
                "creation_method",
                "source_template_id",
                "source_template_version",
                "lifecycle",
                "created_at",
                "updated_at",
                "completed_at",
            ][..],
        ),
        (
            "variants",
            &[
                "variant_id",
                "pet_id",
                "style_id",
                "manifest_path",
                "created_at",
            ][..],
        ),
        ("state", &["key", "value"][..]),
        (
            "identity_profiles",
            &[
                "profile_id",
                "pet_id",
                "schema_version",
                "species",
                "identity_mode",
                "locked_traits",
                "ref_asset_id",
                "created_at",
            ][..],
        ),
        (
            "generation_jobs",
            &[
                "job_id",
                "pet_id",
                "prompt",
                "ref_sha256",
                "task_id",
                "status",
                "result_url",
                "error",
                "created_at",
                "session_id",
            ][..],
        ),
        (
            "appearance_variants",
            &[
                "variant_id",
                "pet_id",
                "job_id",
                "image_path",
                "cutout_path",
                "quality",
                "accepted",
                "created_at",
                "session_id",
                "motion_profile_path",
            ][..],
        ),
        (
            "creation_sessions",
            &[
                "session_id",
                "pet_id",
                "method",
                "status",
                "last_stable_status",
                "current_step",
                "schema_version",
                "error",
                "created_at",
                "updated_at",
                "completed_at",
            ][..],
        ),
        (
            "composer_recipes",
            &[
                "session_id",
                "recipe_version",
                "pack_id",
                "pack_version",
                "layer_contract_version",
                "body_id",
                "ears_id",
                "eyes_id",
                "muzzle_id",
                "tail_id",
                "color_id",
                "pattern_id",
                "updated_at",
            ][..],
        ),
        (
            "creation_session_tombstones",
            &["session_id", "pet_id", "method", "abandoned_at"][..],
        ),
        (
            "creation_upload_sources",
            &[
                "session_id",
                "normalized_png",
                "sha256",
                "mime_type",
                "byte_size",
                "created_at",
            ][..],
        ),
        (
            "creation_adoption_provenance",
            &[
                "session_id",
                "source_template_id",
                "source_template_version",
                "runtime_schema_version",
                "body_sha256",
                "motion_profile_sha256",
                "created_at",
            ][..],
        ),
    ] {
        require_columns(db, table, columns)?;
        require_reference_columns(db, &reference, table, columns)?;
        require_reference_foreign_keys(db, &reference, table)?;
    }

    for (object_type, name) in [
        ("table", "creation_sessions"),
        ("table", "composer_recipes"),
        ("table", "creation_session_tombstones"),
        ("table", "creation_upload_sources"),
        ("table", "creation_adoption_provenance"),
        ("index", "generation_jobs_session_idx"),
        ("index", "appearance_variants_session_idx"),
        ("index", "pets_unique_adoption_source"),
        ("index", "creation_one_long_draft"),
        ("trigger", "pets_validate_source_template_insert"),
        ("trigger", "pets_validate_source_template_update"),
    ] {
        require_reference_object(db, &reference, object_type, name)?;
    }
    validate_pet_creation_method_contract(db)?;

    let foreign_key_errors: i64 = db
        .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })
        .map_err(|error| error.to_string())?;
    if foreign_key_errors != 0 {
        return Err(format!(
            "schema validation failed: {foreign_key_errors} foreign key violation(s)"
        ));
    }
    Ok(())
}

fn validate_latest_schema(db: &Connection) -> Result<(), String> {
    validate_unified_schema(db)?;
    let reference = Connection::open_in_memory().map_err(|error| error.to_string())?;
    for migration in MIGRATIONS {
        reference
            .execute_batch(migration)
            .map_err(|error| format!("failed to build reference schema: {error}"))?;
    }
    require_reference_columns(
        db,
        &reference,
        "appearance_variants",
        &["quality_report_json"],
    )?;
    let profile_columns = [
        "pet_id",
        "schema_version",
        "gender_code",
        "birth_date",
        "updated_at",
    ];
    require_reference_columns(db, &reference, "pet_profiles", &profile_columns)?;
    require_reference_foreign_keys(db, &reference, "pet_profiles")?;
    require_reference_object(db, &reference, "table", "pet_profiles")?;

    for (table, columns) in [
        (
            "photo_avatar_consents",
            &[
                "consent_version",
                "provider_id",
                "disclosure_sha256",
                "accepted_at",
            ] as &[&str],
        ),
        (
            "photo_avatar_sources",
            &[
                "session_id",
                "source_id",
                "ordinal",
                "normalized_png",
                "sha256",
                "width",
                "height",
                "byte_size",
                "created_at",
            ],
        ),
        (
            "photo_avatar_runs",
            &[
                "session_id",
                "revision",
                "style_profile_id",
                "step",
                "provider_session_id",
                "provider_job_id",
                "generation_token",
                "modification_instruction",
                "locked_trait_keys_json",
                "error_code",
                "error_message",
                "updated_at",
            ],
        ),
        (
            "photo_avatar_step_attempts",
            &[
                "session_id",
                "revision",
                "step",
                "attempt_no",
                "provider_job_id",
                "status",
                "retryable",
                "error_code",
                "started_at",
                "finished_at",
            ],
        ),
        (
            "photo_avatar_profiles",
            &[
                "session_id",
                "revision",
                "schema_version",
                "body_module_id",
                "profile_json",
                "profile_sha256",
                "created_at",
            ],
        ),
        (
            "photo_avatar_artifacts",
            &[
                "session_id",
                "revision",
                "kind",
                "relative_path",
                "sha256",
                "local_path",
                "audit_json",
                "created_at",
            ],
        ),
        (
            "photo_avatar_cleanup_audit",
            &[
                "session_id",
                "revision",
                "local_cleanup",
                "backend_cleanup",
                "upstream_cleanup",
                "provider_id",
                "updated_at",
            ],
        ),
    ] {
        require_reference_columns(db, &reference, table, columns)?;
        require_reference_object(db, &reference, "table", table)?;
    }
    for table in [
        "photo_avatar_sources",
        "photo_avatar_runs",
        "photo_avatar_step_attempts",
        "photo_avatar_profiles",
        "photo_avatar_artifacts",
    ] {
        require_reference_foreign_keys(db, &reference, table)?;
    }
    require_reference_object(db, &reference, "index", "photo_avatar_attempt_lookup")
}

fn backfill_legacy_pet_gender(db: &Connection) -> Result<(), String> {
    if !has_columns(db, "pets", &["gender"])? {
        return Ok(());
    }
    db.execute_batch(
        "UPDATE pet_profiles
         SET gender_code = (
           SELECT CASE lower(trim(pets.gender))
             WHEN 'male' THEN 'male'
             WHEN 'female' THEN 'female'
             ELSE NULL
           END
           FROM pets
           WHERE pets.pet_id = pet_profiles.pet_id
         );",
    )
    .map_err(|error| error.to_string())
}

fn is_known_legacy_version_collision(db: &Connection, current: i64) -> Result<bool, String> {
    if !(3..=5).contains(&current) || table_exists(db, "creation_sessions")? {
        return Ok(false);
    }
    if !has_columns(db, "generation_jobs", &["kind"])? {
        return Ok(false);
    }
    let has_complete_profile =
        has_columns(db, "pets", &["name", "gender", "age", "source", "breed"])?;
    let has_upload_source = table_exists(db, "creation_upload_sources")?;
    let has_provenance = table_exists(db, "creation_adoption_provenance")?;
    Ok(match current {
        3 => !has_upload_source && !has_provenance,
        4 => has_complete_profile && !has_upload_source && !has_provenance,
        5 => has_provenance && (has_complete_profile || has_upload_source),
        _ => false,
    })
}

fn repair_legacy_version_collision(db: &Connection, current: i64) -> Result<(), String> {
    if !is_known_legacy_version_collision(db, current)? {
        return Ok(());
    }

    let has_legacy_name = has_columns(db, "pets", &["name"])?;
    let has_upload_source = table_exists(db, "creation_upload_sources")?;
    let has_provenance = table_exists(db, "creation_adoption_provenance")?;
    db.execute_batch("BEGIN IMMEDIATE;")
        .map_err(|error| error.to_string())?;
    let repair = (|| {
        db.execute_batch(MIGRATIONS[2])
            .map_err(|error| error.to_string())?;
        if has_legacy_name {
            db.execute_batch("UPDATE pets SET display_name=name WHERE trim(name) != '';")
                .map_err(|error| error.to_string())?;
        }
        if !has_upload_source {
            db.execute_batch(MIGRATIONS[3])
                .map_err(|error| error.to_string())?;
        }
        if !has_provenance {
            db.execute_batch(MIGRATIONS[4])
                .map_err(|error| error.to_string())?;
        }
        validate_unified_schema(db)?;
        db.execute_batch(MIGRATIONS[5])
            .map_err(|error| error.to_string())?;
        db.pragma_update(None, "user_version", LEGACY_REPAIR_TARGET_VERSION)
            .map_err(|error| error.to_string())?;
        db.execute_batch("COMMIT;")
            .map_err(|error| error.to_string())
    })();
    if let Err(error) = repair {
        let _ = db.execute_batch("ROLLBACK;");
        return Err(format!(
            "legacy migration version collision repair failed: {error}"
        ));
    }
    Ok(())
}

fn apply_migration(db: &Connection, index: usize, migration: &str) -> Result<(), String> {
    let version = index + 1;
    db.execute_batch("BEGIN IMMEDIATE;")
        .map_err(|error| format!("migration {index} failed: {error}"))?;
    let result = (|| {
        if version == UNIFIED_CREATION_SCHEMA_VERSION {
            validate_unified_schema(db)?;
        }
        db.execute_batch(migration)
            .map_err(|error| error.to_string())?;
        if version == PET_PROFILE_SCHEMA_VERSION {
            backfill_legacy_pet_gender(db)?;
        }
        if version == MIGRATIONS.len() {
            validate_latest_schema(db)?;
        }
        db.pragma_update(None, "user_version", version as i64)
            .map_err(|error| error.to_string())?;
        db.execute_batch("COMMIT;")
            .map_err(|error| error.to_string())
    })();
    if let Err(error) = result {
        let _ = db.execute_batch("ROLLBACK;");
        return Err(format!("migration {index} failed: {error}"));
    }
    Ok(())
}

pub fn apply(db: &Connection) -> Result<(), String> {
    let mut current: i64 = db
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    repair_legacy_version_collision(db, current)?;
    current = db
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if current as usize == MIGRATIONS.len() {
        return validate_latest_schema(db);
    }
    if current as usize > MIGRATIONS.len() {
        return Ok(());
    }
    if current as usize == UNIFIED_CREATION_SCHEMA_VERSION {
        validate_unified_schema(db)?;
    }
    for (index, migration) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        apply_migration(db, index, migration)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_db() -> (Connection, std::path::PathBuf) {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "desktop-pet-migrate-{}-{n}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        (Connection::open(root.join("m.db")).unwrap(), root)
    }

    fn insert_v2_pet(db: &Connection, pet_id: &str, identity_mode: &str, created_at: &str) {
        db.execute(
            "INSERT INTO pets
             (pet_id, schema_version, species, identity_mode, created_at, updated_at)
             VALUES (?1, 1, 'cat', ?2, ?3, ?3)",
            rusqlite::params![pet_id, identity_mode, created_at],
        )
        .unwrap();
    }

    fn apply_through(db: &Connection, version: usize) {
        assert!(version <= MIGRATIONS.len());
        for (index, migration) in MIGRATIONS.iter().enumerate().take(version) {
            db.execute_batch(migration).unwrap();
            db.pragma_update(None, "user_version", (index + 1) as i64)
                .unwrap();
        }
    }

    fn insert_ready_completed_pet(db: &Connection, pet_id: &str) {
        db.execute(
            "INSERT INTO pets
             (pet_id, schema_version, species, identity_mode, display_name,
              creation_method, lifecycle, created_at, updated_at, completed_at)
             VALUES (?1, 1, 'cat', 'realpet', ?1, 'upload', 'ready',
                     '2026-01-01T00:00:00Z', '2026-01-02T00:00:00Z',
                     '2026-01-02T00:00:00Z')",
            [pet_id],
        )
        .unwrap();
    }

    fn profile(db: &Connection, pet_id: &str) -> (Option<String>, Option<String>) {
        db.query_row(
            "SELECT gender_code, birth_date FROM pet_profiles WHERE pet_id=?1",
            [pet_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap()
    }

    const LEGACY_V3_JOB_KIND: &str = r#"
        ALTER TABLE generation_jobs ADD COLUMN kind TEXT NOT NULL DEFAULT 'main';
    "#;

    const LEGACY_V4_PET_PROFILE: &str = r#"
        ALTER TABLE pets ADD COLUMN name TEXT NOT NULL DEFAULT '';
        ALTER TABLE pets ADD COLUMN gender TEXT NOT NULL DEFAULT '';
        ALTER TABLE pets ADD COLUMN age TEXT NOT NULL DEFAULT '';
        ALTER TABLE pets ADD COLUMN source TEXT NOT NULL DEFAULT '';
        ALTER TABLE pets ADD COLUMN breed TEXT NOT NULL DEFAULT '';
    "#;

    fn prepare_legacy_collision_database(db: &Connection, legacy_version: i64) {
        db.execute_batch(MIGRATIONS[0]).unwrap();
        db.execute_batch(MIGRATIONS[1]).unwrap();
        db.execute_batch(LEGACY_V3_JOB_KIND).unwrap();
        if legacy_version >= 4 {
            db.execute_batch(LEGACY_V4_PET_PROFILE).unwrap();
        }
        if legacy_version >= 5 {
            db.execute_batch(MIGRATIONS[4]).unwrap();
        }
        db.pragma_update(None, "user_version", legacy_version)
            .unwrap();

        insert_v2_pet(db, "pet-legacy", "reference", "10");
        if legacy_version >= 4 {
            db.execute(
                "UPDATE pets SET name='Legacy Cat', gender='female', age='3',
                 source='local', breed='tabby' WHERE pet_id='pet-legacy'",
                [],
            )
            .unwrap();
        }
        db.execute(
            "INSERT INTO generation_jobs
             (job_id, pet_id, prompt, ref_sha256, status, created_at, kind)
             VALUES ('job-legacy', 'pet-legacy', 'prompt', 'hash', 'succeeded', '10',
                     'eyeClosed')",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO appearance_variants
             (variant_id, pet_id, job_id, image_path, cutout_path, quality, accepted, created_at)
             VALUES ('variant-legacy', 'pet-legacy', 'job-legacy', 'image.png',
                     'C:\\jobs\\job-legacy\\cutout.png', 'ok', 1, '10')",
            [],
        )
        .unwrap();
        db.pragma_update(None, "foreign_keys", "ON").unwrap();
    }

    fn insert_draft_pet_and_session(
        db: &Connection,
        pet_id: &str,
        session_id: &str,
        method: &str,
    ) -> rusqlite::Result<usize> {
        db.execute(
            "INSERT INTO pets
             (pet_id, schema_version, species, identity_mode, creation_method,
              lifecycle, created_at, updated_at)
             VALUES (?1, 1, 'cat', 'realpet', ?3, 'draft', '10', '10')",
            rusqlite::params![pet_id, session_id, method],
        )?;
        db.execute(
            "INSERT INTO creation_sessions
             (session_id, pet_id, method, status, last_stable_status, current_step,
              schema_version, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'draft', 'draft', 'upload', 1, '10', '10')",
            rusqlite::params![session_id, pet_id, method],
        )
    }

    #[test]
    fn migrates_empty_database_to_latest() {
        let (db, root) = temp_db();
        apply(&db).unwrap();
        let version: i64 = db
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as i64);
        let tables: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('pets','variants','state')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tables, 3);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn repairs_legacy_version_collisions_without_losing_rows() {
        for legacy_version in [3_i64, 4, 5] {
            let (db, root) = temp_db();
            prepare_legacy_collision_database(&db, legacy_version);

            apply(&db).unwrap();

            let version: i64 = db
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .unwrap();
            assert_eq!(version, MIGRATIONS.len() as i64, "legacy v{legacy_version}");
            let preserved: (i64, i64, i64) = db
                .query_row(
                    "SELECT (SELECT COUNT(*) FROM pets),
                            (SELECT COUNT(*) FROM generation_jobs),
                            (SELECT COUNT(*) FROM appearance_variants)",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .unwrap();
            assert_eq!(preserved, (1, 1, 1), "legacy v{legacy_version}");
            let migrated: (String, String, String, String) = db
                .query_row(
                    "SELECT cs.status, gj.session_id, av.session_id, gj.kind
                     FROM creation_sessions cs
                     JOIN generation_jobs gj ON gj.pet_id=cs.pet_id
                     JOIN appearance_variants av ON av.pet_id=cs.pet_id
                     WHERE cs.pet_id='pet-legacy'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .unwrap();
            assert_eq!(
                migrated,
                (
                    "completed".into(),
                    "session-migrated-pet-legacy".into(),
                    "session-migrated-pet-legacy".into(),
                    "eyeClosed".into(),
                ),
                "legacy v{legacy_version}"
            );
            if legacy_version >= 4 {
                let profile: (String, String, String, String, String, String) = db
                    .query_row(
                        "SELECT display_name, name, gender, age, source, breed
                         FROM pets WHERE pet_id='pet-legacy'",
                        [],
                        |row| {
                            Ok((
                                row.get(0)?,
                                row.get(1)?,
                                row.get(2)?,
                                row.get(3)?,
                                row.get(4)?,
                                row.get(5)?,
                            ))
                        },
                    )
                    .unwrap();
                assert_eq!(
                    profile,
                    (
                        "Legacy Cat".into(),
                        "Legacy Cat".into(),
                        "female".into(),
                        "3".into(),
                        "local".into(),
                        "tabby".into(),
                    )
                );
            }
            for table in [
                "creation_sessions",
                "composer_recipes",
                "creation_session_tombstones",
                "creation_upload_sources",
                "creation_adoption_provenance",
            ] {
                let exists: bool = db
                    .query_row(
                        "SELECT EXISTS(
                           SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1
                         )",
                        [table],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert!(exists, "legacy v{legacy_version} missing {table}");
            }
            let foreign_key_errors: i64 = db
                .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(foreign_key_errors, 0, "legacy v{legacy_version}");

            apply(&db).unwrap();
            let _ = std::fs::remove_dir_all(root);
        }
    }

    #[test]
    fn legacy_collision_repairs_to_v6_then_runs_later_migrations() {
        let (db, root) = temp_db();
        db.execute_batch(MIGRATIONS[0]).unwrap();
        db.execute_batch(MIGRATIONS[1]).unwrap();
        db.execute_batch(LEGACY_V3_JOB_KIND).unwrap();
        db.pragma_update(None, "user_version", 3).unwrap();

        apply(&db).unwrap();

        let version: i64 = db
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as i64);
        assert!(has_columns(&db, "appearance_variants", &["quality_report_json"]).unwrap());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn v8_creates_profiles_and_backfills_only_known_legacy_gender() {
        let (db, root) = temp_db();
        apply_through(&db, 7);
        db.execute_batch(
            "ALTER TABLE pets ADD COLUMN gender TEXT;
             ALTER TABLE pets ADD COLUMN age INTEGER;",
        )
        .unwrap();
        insert_ready_completed_pet(&db, "pet-a");
        insert_ready_completed_pet(&db, "pet-b");
        db.execute(
            "UPDATE pets SET gender='female', age=3 WHERE pet_id='pet-a'",
            [],
        )
        .unwrap();
        db.execute(
            "UPDATE pets SET gender='other-value', age=7 WHERE pet_id='pet-b'",
            [],
        )
        .unwrap();

        apply(&db).unwrap();

        let version: i64 = db
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as i64);
        assert_eq!(profile(&db, "pet-a"), (Some("female".into()), None));
        assert_eq!(profile(&db, "pet-b"), (None, None));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn v8_new_database_has_exact_profile_columns() {
        let (db, root) = temp_db();

        apply(&db).unwrap();

        let version: i64 = db
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as i64);
        let columns = {
            let mut statement = db.prepare("PRAGMA table_info(pet_profiles)").unwrap();
            statement
                .query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(
            columns,
            [
                "pet_id",
                "schema_version",
                "gender_code",
                "birth_date",
                "updated_at"
            ]
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn v8_birth_date_check_enforces_the_gregorian_calendar() {
        let (db, root) = temp_db();
        db.pragma_update(None, "foreign_keys", "ON").unwrap();
        apply(&db).unwrap();

        for (index, date) in ["2000-02-29", "1900-02-28", "9999-12-31"]
            .into_iter()
            .enumerate()
        {
            let pet_id = format!("pet-valid-{index}");
            insert_ready_completed_pet(&db, &pet_id);
            db.execute(
                "INSERT INTO pet_profiles
                 (pet_id, schema_version, gender_code, birth_date, updated_at)
                 VALUES (?1, 1, NULL, ?2, 'now')",
                rusqlite::params![pet_id, date],
            )
            .unwrap();
        }

        for (index, date) in [
            "2025-02-29",
            "1900-02-29",
            "0000-00-00",
            "2024-04-31",
            "2024-13-01",
            "２０２４-０２-２９",
        ]
        .into_iter()
        .enumerate()
        {
            let pet_id = format!("pet-invalid-{index}");
            insert_ready_completed_pet(&db, &pet_id);
            assert!(
                db.execute(
                    "INSERT INTO pet_profiles
                     (pet_id, schema_version, gender_code, birth_date, updated_at)
                     VALUES (?1, 1, NULL, ?2, 'now')",
                    rusqlite::params![pet_id, date],
                )
                .is_err(),
                "INSERT accepted {date}"
            );
        }

        assert!(db
            .execute(
                "UPDATE pet_profiles SET birth_date='2025-02-29'
                 WHERE pet_id='pet-valid-0'",
                [],
            )
            .is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn v8_rejects_a_weak_birth_date_check_in_latest_schema() {
        let (db, root) = temp_db();
        apply_through(&db, 7);
        db.execute_batch(
            "CREATE TABLE pet_profiles (
               pet_id TEXT PRIMARY KEY REFERENCES pets(pet_id) ON DELETE CASCADE,
               schema_version INTEGER NOT NULL DEFAULT 1 CHECK(schema_version = 1),
               gender_code TEXT CHECK(gender_code IN ('male','female') OR gender_code IS NULL),
               birth_date TEXT CHECK(birth_date IS NULL OR birth_date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]'),
               updated_at TEXT NOT NULL
             );",
        )
        .unwrap();
        db.pragma_update(None, "user_version", 8).unwrap();

        let error = apply(&db).unwrap_err();

        assert!(error.contains("pet_profiles"), "{error}");
        assert!(error.contains("wrong definition"), "{error}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn v8_creates_profiles_for_all_ready_completed_pets_without_legacy_gender() {
        let (db, root) = temp_db();
        apply_through(&db, 7);
        insert_ready_completed_pet(&db, "pet-completed-a");
        insert_ready_completed_pet(&db, "pet-completed-b");
        db.execute(
            "INSERT INTO pets
             (pet_id, schema_version, species, identity_mode, display_name,
              creation_method, lifecycle, created_at, updated_at)
             VALUES ('pet-draft', 1, 'cat', 'realpet', 'Draft', 'upload',
                     'draft', '2026-01-01T00:00:00Z', '2026-01-03T00:00:00Z')",
            [],
        )
        .unwrap();

        apply(&db).unwrap();

        let rows: Vec<(String, i64, Option<String>, Option<String>, String)> = {
            let mut statement = db
                .prepare(
                    "SELECT pet_id, schema_version, gender_code, birth_date, updated_at
                     FROM pet_profiles ORDER BY pet_id",
                )
                .unwrap();
            statement
                .query_map([], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(
            rows,
            vec![
                (
                    "pet-completed-a".into(),
                    1,
                    None,
                    None,
                    "2026-01-02T00:00:00Z".into()
                ),
                (
                    "pet-completed-b".into(),
                    1,
                    None,
                    None,
                    "2026-01-02T00:00:00Z".into()
                )
            ]
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn v8_trims_and_lowercases_gender_without_inferring_unknown_or_age() {
        let (db, root) = temp_db();
        apply_through(&db, 7);
        db.execute_batch(
            "ALTER TABLE pets ADD COLUMN gender TEXT;
             ALTER TABLE pets ADD COLUMN age INTEGER;",
        )
        .unwrap();
        for (pet_id, gender, age) in [
            ("pet-male", "  MaLe ", 2),
            ("pet-female", " FEMALE  ", 4),
            ("pet-unknown", " unknown ", 6),
            ("pet-invalid", "nonbinary", 8),
            ("pet-empty", "   ", 10),
        ] {
            insert_ready_completed_pet(&db, pet_id);
            db.execute(
                "UPDATE pets SET gender=?2, age=?3 WHERE pet_id=?1",
                rusqlite::params![pet_id, gender, age],
            )
            .unwrap();
        }

        apply(&db).unwrap();

        assert_eq!(profile(&db, "pet-male"), (Some("male".into()), None));
        assert_eq!(profile(&db, "pet-female"), (Some("female".into()), None));
        for pet_id in ["pet-unknown", "pet-invalid", "pet-empty"] {
            assert_eq!(profile(&db, pet_id), (None, None), "{pet_id}");
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn v8_reapply_is_idempotent() {
        let (db, root) = temp_db();
        apply_through(&db, 7);
        db.execute_batch("ALTER TABLE pets ADD COLUMN gender TEXT;")
            .unwrap();
        insert_ready_completed_pet(&db, "pet-a");
        db.execute("UPDATE pets SET gender='male' WHERE pet_id='pet-a'", [])
            .unwrap();

        apply(&db).unwrap();
        apply(&db).unwrap();

        let row: (i64, Option<String>) = db
            .query_row(
                "SELECT COUNT(*), MAX(gender_code) FROM pet_profiles WHERE pet_id='pet-a'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(row, (1, Some("male".into())));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn v8_profile_is_deleted_with_its_pet() {
        let (db, root) = temp_db();
        db.pragma_update(None, "foreign_keys", "ON").unwrap();
        apply_through(&db, 7);
        insert_ready_completed_pet(&db, "pet-a");
        apply(&db).unwrap();

        db.execute("DELETE FROM pets WHERE pet_id='pet-a'", [])
            .unwrap();

        let profiles: i64 = db
            .query_row("SELECT COUNT(*) FROM pet_profiles", [], |row| row.get(0))
            .unwrap();
        assert_eq!(profiles, 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn v8_rejects_latest_schema_missing_a_profile_column() {
        let (db, root) = temp_db();
        apply_through(&db, 7);
        db.execute_batch(
            "ALTER TABLE pets ADD COLUMN gender TEXT;
             ALTER TABLE pets ADD COLUMN age INTEGER;
             CREATE TABLE pet_profiles (
               pet_id TEXT PRIMARY KEY REFERENCES pets(pet_id) ON DELETE CASCADE,
               schema_version INTEGER NOT NULL DEFAULT 1 CHECK(schema_version = 1),
               gender_code TEXT CHECK(gender_code IN ('male','female') OR gender_code IS NULL),
               updated_at TEXT NOT NULL
             );",
        )
        .unwrap();
        db.pragma_update(None, "user_version", 8).unwrap();

        let error = apply(&db).unwrap_err();

        assert!(error.contains("pet_profiles.birth_date"), "{error}");
        let version: i64 = db
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as i64 - 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn v8_rolls_back_as_one_transaction_when_profile_table_conflicts() {
        let (db, root) = temp_db();
        apply_through(&db, 7);
        db.execute_batch("CREATE TABLE pet_profiles (wrong TEXT);")
            .unwrap();

        let error = apply(&db).unwrap_err();

        assert!(error.contains("migration 7"), "{error}");
        let version: i64 = db
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 7);
        let columns: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('pet_profiles')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(columns, 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn v8_rolls_back_when_latest_validation_rejects_damaged_v7_schema() {
        let (db, root) = temp_db();
        apply_through(&db, 7);
        db.execute_batch("ALTER TABLE appearance_variants DROP COLUMN quality_report_json;")
            .unwrap();

        let error = apply(&db).unwrap_err();

        assert!(
            error.contains("appearance_variants.quality_report_json"),
            "{error}"
        );
        let version: i64 = db
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as i64 - 1);
        assert!(table_exists(&db, "pet_profiles").unwrap());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn v7_is_idempotent_and_preserves_legacy_profile_columns() {
        let (db, root) = temp_db();
        db.execute_batch(MIGRATIONS[0]).unwrap();
        db.execute_batch(MIGRATIONS[1]).unwrap();
        db.execute_batch(LEGACY_V3_JOB_KIND).unwrap();
        db.execute_batch(LEGACY_V4_PET_PROFILE).unwrap();
        db.pragma_update(None, "user_version", 4).unwrap();
        apply(&db).unwrap();
        apply(&db).unwrap();
        assert!(has_columns(&db, "pets", &["gender", "age"]).unwrap());
        assert!(has_columns(&db, "appearance_variants", &["quality_report_json"]).unwrap());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_unknown_latest_version_schema_when_required_tables_are_missing() {
        let (db, root) = temp_db();
        db.execute_batch(MIGRATIONS[0]).unwrap();
        db.execute_batch(MIGRATIONS[1]).unwrap();
        db.pragma_update(None, "user_version", 5).unwrap();

        let error = apply(&db).unwrap_err();

        assert!(error.contains("schema validation failed"), "{error}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn repairs_legacy_v3_already_advanced_to_current_v5() {
        let (db, root) = temp_db();
        db.execute_batch(MIGRATIONS[0]).unwrap();
        db.execute_batch(MIGRATIONS[1]).unwrap();
        db.execute_batch(LEGACY_V3_JOB_KIND).unwrap();
        db.execute_batch(MIGRATIONS[3]).unwrap();
        db.execute_batch(MIGRATIONS[4]).unwrap();
        db.pragma_update(None, "user_version", 5).unwrap();
        insert_v2_pet(&db, "pet-v3-advanced", "reference", "10");
        db.execute(
            "INSERT INTO generation_jobs
             (job_id, pet_id, prompt, ref_sha256, status, created_at, kind)
             VALUES ('job-v3-advanced', 'pet-v3-advanced', 'prompt', 'hash',
                     'succeeded', '10', 'eyeClosed')",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO appearance_variants
             (variant_id, pet_id, job_id, image_path, quality, accepted, created_at)
             VALUES ('variant-v3-advanced', 'pet-v3-advanced', 'job-v3-advanced',
                     'image.png', 'ok', 1, '10')",
            [],
        )
        .unwrap();
        db.pragma_update(None, "foreign_keys", "ON").unwrap();

        apply(&db).unwrap();

        let migrated: (i64, String, String) = db
            .query_row(
                "SELECT (SELECT COUNT(*) FROM pets), cs.status, gj.kind
                 FROM creation_sessions cs
                 JOIN generation_jobs gj ON gj.pet_id=cs.pet_id
                 WHERE cs.pet_id='pet-v3-advanced'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(migrated, (1, "completed".into(), "eyeClosed".into()));
        let version: i64 = db
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as i64);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn maps_legacy_name_when_profile_columns_exist_at_version_three() {
        let (db, root) = temp_db();
        db.execute_batch(MIGRATIONS[0]).unwrap();
        db.execute_batch(MIGRATIONS[1]).unwrap();
        db.execute_batch(LEGACY_V3_JOB_KIND).unwrap();
        db.execute_batch(LEGACY_V4_PET_PROFILE).unwrap();
        db.pragma_update(None, "user_version", 3).unwrap();
        insert_v2_pet(&db, "pet-profile-before-version", "reference", "10");
        db.execute(
            "UPDATE pets SET name='Crash Safe Name'
             WHERE pet_id='pet-profile-before-version'",
            [],
        )
        .unwrap();

        apply(&db).unwrap();

        let names: (String, String) = db
            .query_row(
                "SELECT display_name, name FROM pets
                 WHERE pet_id='pet-profile-before-version'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(names, ("Crash Safe Name".into(), "Crash Safe Name".into()));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rolls_back_legacy_repair_when_an_existing_current_table_is_malformed() {
        let (db, root) = temp_db();
        db.execute_batch(MIGRATIONS[0]).unwrap();
        db.execute_batch(MIGRATIONS[1]).unwrap();
        db.execute_batch(LEGACY_V3_JOB_KIND).unwrap();
        db.execute_batch("CREATE TABLE creation_upload_sources (session_id TEXT PRIMARY KEY);")
            .unwrap();
        db.execute_batch(MIGRATIONS[4]).unwrap();
        db.pragma_update(None, "user_version", 5).unwrap();
        insert_v2_pet(&db, "pet-rollback", "reference", "10");

        let error = apply(&db).unwrap_err();

        assert!(error.contains("schema validation"), "{error}");
        let version: i64 = db
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 5);
        assert!(!table_exists(&db, "creation_sessions").unwrap());
        assert!(!has_columns(&db, "pets", &["display_name"]).unwrap());
        let pet_count: i64 = db
            .query_row("SELECT COUNT(*) FROM pets", [], |row| row.get(0))
            .unwrap();
        assert_eq!(pet_count, 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_incomplete_unified_schema_at_v5_and_v6() {
        for version in [5_i64, 6] {
            let (db, root) = temp_db();
            for migration in &MIGRATIONS[..5] {
                db.execute_batch(migration).unwrap();
            }
            db.execute_batch("ALTER TABLE creation_sessions DROP COLUMN error;")
                .unwrap();
            db.pragma_update(None, "user_version", version).unwrap();

            let error = apply(&db).unwrap_err();

            assert!(
                error.contains("creation_sessions.error"),
                "v{version}: {error}"
            );
            let _ = std::fs::remove_dir_all(root);
        }
    }

    #[test]
    fn rejects_unconstrained_adoption_provenance_table() {
        let (db, root) = temp_db();
        for migration in &MIGRATIONS[..4] {
            db.execute_batch(migration).unwrap();
        }
        db.execute_batch(
            "CREATE TABLE creation_adoption_provenance (
               session_id TEXT,
               source_template_id TEXT,
               source_template_version INTEGER,
               runtime_schema_version INTEGER,
               body_sha256 TEXT,
               motion_profile_sha256 TEXT,
               created_at TEXT
             );",
        )
        .unwrap();
        db.pragma_update(None, "user_version", 5).unwrap();

        let error = apply(&db).unwrap_err();

        assert!(error.contains("creation_adoption_provenance"), "{error}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_unified_schema_missing_required_index_or_trigger() {
        for (drop_statement, expected) in [
            (
                "DROP INDEX creation_one_long_draft;",
                "creation_one_long_draft",
            ),
            (
                "DROP TRIGGER pets_validate_source_template_insert;",
                "pets_validate_source_template_insert",
            ),
        ] {
            let (db, root) = temp_db();
            for migration in &MIGRATIONS[..5] {
                db.execute_batch(migration).unwrap();
            }
            db.execute_batch(drop_statement).unwrap();
            db.pragma_update(None, "user_version", 5).unwrap();

            let error = apply(&db).unwrap_err();

            assert!(error.contains(expected), "{error}");
            let _ = std::fs::remove_dir_all(root);
        }
    }

    #[test]
    fn rejects_semantically_wrong_unified_index() {
        let (db, root) = temp_db();
        for migration in &MIGRATIONS[..5] {
            db.execute_batch(migration).unwrap();
        }
        db.execute_batch(
            "DROP INDEX pets_unique_adoption_source;
             CREATE UNIQUE INDEX pets_unique_adoption_source
               ON pets(pet_id)
               WHERE source_template_id IS NOT NULL;",
        )
        .unwrap();
        db.pragma_update(None, "user_version", 5).unwrap();

        let error = apply(&db).unwrap_err();

        assert!(error.contains("pets_unique_adoption_source"), "{error}");
        let version: i64 = db
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 5);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_semantically_wrong_constraints_and_trigger_predicates() {
        for (replacement, expected) in [
            (
                "DROP INDEX creation_one_long_draft;
                 CREATE UNIQUE INDEX creation_one_long_draft
                   ON creation_sessions ((1))
                   WHERE status NOT IN ('completed','abandoned');",
                "creation_one_long_draft",
            ),
            (
                "DROP TRIGGER pets_validate_source_template_insert;
                 CREATE TRIGGER pets_validate_source_template_insert
                   BEFORE INSERT ON pets
                   WHEN NEW.source_template_id IS NULL
                 BEGIN
                   SELECT RAISE(ABORT, 'invalid pet source template');
                 END;",
                "pets_validate_source_template_insert",
            ),
            (
                "DROP TABLE creation_upload_sources;
                 CREATE TABLE creation_upload_sources (
                   session_id TEXT PRIMARY KEY
                     REFERENCES creation_sessions(session_id) ON DELETE CASCADE,
                   normalized_png BLOB NOT NULL,
                   sha256 TEXT NOT NULL,
                   mime_type TEXT NOT NULL,
                   byte_size INTEGER NOT NULL CHECK(byte_size <= 10485760),
                   created_at TEXT NOT NULL,
                   CHECK(length(normalized_png) = byte_size)
                 );",
                "creation_upload_sources",
            ),
            (
                "DROP TABLE creation_upload_sources;
                 CREATE TABLE creation_upload_sources (
                   session_id TEXT PRIMARY KEY
                     REFERENCES creation_sessions(session_id) ON DELETE CASCADE,
                   normalized_png BLOB NOT NULL,
                   sha256 TEXT NOT NULL CHECK(length(sha256) = 64),
                   mime_type TEXT NOT NULL CHECK(mime_type = 'IMAGE/PNG'),
                   byte_size INTEGER NOT NULL CHECK(byte_size > 0 AND byte_size <= 10485760),
                   created_at TEXT NOT NULL,
                   CHECK(length(normalized_png) = byte_size)
                 );",
                "creation_upload_sources",
            ),
            (
                "DROP TABLE creation_adoption_provenance;
                 CREATE TABLE creation_adoption_provenance (
                   session_id TEXT PRIMARY KEY
                     REFERENCES creation_sessions(session_id) ON DELETE CASCADE,
                   source_template_id TEXT NOT NULL,
                   source_template_version INTEGER NOT NULL CHECK(source_template_version > 0),
                   runtime_schema_version INTEGER NOT NULL CHECK(runtime_schema_version > 0),
                   body_sha256 TEXT NOT NULL CHECK(length(body_sha256) = 64),
                   motion_profile_sha256 TEXT NOT NULL
                     CHECK(length(motion_profile_sha256) = 64),
                   created_at TEXT NOT NULL
                 );",
                "creation_adoption_provenance",
            ),
        ] {
            let (db, root) = temp_db();
            for migration in &MIGRATIONS[..5] {
                db.execute_batch(migration).unwrap();
            }
            db.execute_batch(replacement).unwrap();
            db.pragma_update(None, "user_version", 5).unwrap();

            let error = apply(&db).unwrap_err();

            assert!(error.contains(expected), "{expected}: {error}");
            let version: i64 = db
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .unwrap();
            assert_eq!(version, 5);
            let _ = std::fs::remove_dir_all(root);
        }
    }

    #[test]
    fn rejects_pets_table_without_creation_method_check() {
        let (db, root) = temp_db();
        for migration in &MIGRATIONS[..5] {
            db.execute_batch(migration).unwrap();
        }
        db.execute_batch(
            "PRAGMA writable_schema=ON;
             UPDATE sqlite_master
             SET sql=replace(
               sql,
               ' CHECK(creation_method IN (''upload'',''composer'',''adoption''))',
               ''
             )
             WHERE type='table' AND name='pets';
             PRAGMA writable_schema=OFF;",
        )
        .unwrap();
        db.pragma_update(None, "user_version", 5).unwrap();

        let error = apply(&db).unwrap_err();

        assert!(error.contains("pets.creation_method CHECK"), "{error}");
        let version: i64 = db
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 5);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_weak_creation_method_check() {
        let (db, root) = temp_db();
        for migration in &MIGRATIONS[..5] {
            db.execute_batch(migration).unwrap();
        }
        db.execute_batch(
            "PRAGMA writable_schema=ON;
             UPDATE sqlite_master
             SET sql=replace(
               sql,
               'CHECK(creation_method IN (''upload'',''composer'',''adoption''))',
               'CHECK(creation_method != ''invalid'')'
             )
             WHERE type='table' AND name='pets';
             PRAGMA writable_schema=OFF;",
        )
        .unwrap();
        db.pragma_update(None, "user_version", 5).unwrap();

        let error = apply(&db).unwrap_err();

        assert!(error.contains("pets.creation_method CHECK"), "{error}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_existing_invalid_creation_method_data() {
        let (db, root) = temp_db();
        for migration in &MIGRATIONS[..5] {
            db.execute_batch(migration).unwrap();
        }
        db.execute_batch(
            "PRAGMA ignore_check_constraints=ON;
             INSERT INTO pets
               (pet_id, schema_version, species, identity_mode, creation_method,
                lifecycle, created_at, updated_at)
             VALUES ('pet-invalid-method', 1, 'cat', 'realpet', 'legacy', 'ready', '1', '1');
             PRAGMA ignore_check_constraints=OFF;",
        )
        .unwrap();
        db.pragma_update(None, "user_version", 5).unwrap();

        let error = apply(&db).unwrap_err();

        assert!(error.contains("invalid creation_method data"), "{error}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_unified_schema_missing_required_base_table() {
        for table in ["variants", "state", "identity_profiles"] {
            let (db, root) = temp_db();
            for migration in &MIGRATIONS[..5] {
                db.execute_batch(migration).unwrap();
            }
            db.execute_batch(&format!("DROP TABLE {table};")).unwrap();
            db.pragma_update(None, "user_version", 5).unwrap();

            let error = apply(&db).unwrap_err();

            assert!(error.contains(table), "{error}");
            let _ = std::fs::remove_dir_all(root);
        }
    }

    #[test]
    fn latest_migration_creates_session_owned_upload_source_blob_table() {
        let (db, root) = temp_db();
        db.pragma_update(None, "foreign_keys", "ON").unwrap();
        apply(&db).unwrap();

        let sql: String = db
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='creation_upload_sources'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        for required in [
            "session_id",
            "normalized_png",
            "sha256",
            "mime_type",
            "byte_size",
            "created_at",
        ] {
            assert!(sql.contains(required), "missing {required}: {sql}");
        }
        assert!(sql.contains("REFERENCES creation_sessions"));
        assert!(sql.contains("ON DELETE CASCADE"));
        assert!(
            sql.contains("byte_size <= 10485760"),
            "missing retransmittable normalized source hard limit: {sql}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn latest_migration_creates_transactional_adoption_provenance_table() {
        let (db, root) = temp_db();
        db.pragma_update(None, "foreign_keys", "ON").unwrap();
        apply(&db).unwrap();

        let sql: String = db
            .query_row(
                "SELECT sql FROM sqlite_master
                 WHERE type='table' AND name='creation_adoption_provenance'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        for required in [
            "session_id",
            "source_template_id",
            "source_template_version",
            "runtime_schema_version",
            "body_sha256",
            "motion_profile_sha256",
            "created_at",
        ] {
            assert!(sql.contains(required), "missing {required}: {sql}");
        }
        assert!(sql.contains("REFERENCES creation_sessions"));
        assert!(sql.contains("ON DELETE CASCADE"));
        assert!(sql.contains("length(body_sha256) = 64"));
        assert!(sql.contains("length(motion_profile_sha256) = 64"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn adoption_provenance_migration_rolls_back_as_one_transaction_on_conflict() {
        let (db, root) = temp_db();
        for migration in &MIGRATIONS[..4] {
            db.execute_batch(migration).unwrap();
        }
        db.pragma_update(None, "user_version", 4).unwrap();
        db.execute_batch("CREATE TABLE creation_adoption_provenance (wrong TEXT);")
            .unwrap();

        let error = apply(&db).unwrap_err();

        assert!(error.contains("migration 4"), "{error}");
        let version: i64 = db
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 4);
        let columns: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('creation_adoption_provenance')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(columns, 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn upload_source_migration_rolls_back_as_one_transaction_on_conflict() {
        let (db, root) = temp_db();
        for migration in &MIGRATIONS[..3] {
            db.execute_batch(migration).unwrap();
        }
        db.pragma_update(None, "user_version", 3).unwrap();
        db.execute_batch("CREATE TABLE creation_upload_sources (wrong TEXT);")
            .unwrap();

        let error = apply(&db).unwrap_err();

        assert!(error.contains("migration 3"));
        let version: i64 = db
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 3);
        let columns: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('creation_upload_sources')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(columns, 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn reapply_is_idempotent() {
        let (db, root) = temp_db();
        apply(&db).unwrap();
        apply(&db).unwrap();
        let version: i64 = db
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as i64);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn upgrades_v2_rows_without_losing_existing_pets() {
        let (db, root) = temp_db();
        db.execute_batch(MIGRATIONS[0]).unwrap();
        db.execute_batch(MIGRATIONS[1]).unwrap();
        db.pragma_update(None, "user_version", 2).unwrap();
        insert_v2_pet(&db, "pet-old", "realpet", "10");

        apply(&db).unwrap();

        let row: (String, String, String, String) = db
            .query_row(
                "SELECT display_name, creation_method, lifecycle, completed_at
                 FROM pets WHERE pet_id='pet-old'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            row,
            (
                "我的猫咪".into(),
                "upload".into(),
                "ready".into(),
                "10".into()
            )
        );
        let session_count: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM creation_sessions WHERE pet_id='pet-old'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(session_count, 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn migrated_sessions_reflect_legacy_identity_and_candidate_state() {
        let (db, root) = temp_db();
        db.execute_batch(MIGRATIONS[0]).unwrap();
        db.execute_batch(MIGRATIONS[1]).unwrap();
        db.pragma_update(None, "user_version", 2).unwrap();
        insert_v2_pet(&db, "pet-guided", "guided", "10");
        insert_v2_pet(&db, "pet-adopted", "adopted", "20");
        insert_v2_pet(&db, "pet-upload", "reference", "30");
        db.execute(
            "INSERT INTO appearance_variants
             (variant_id, pet_id, image_path, quality, accepted, created_at)
             VALUES ('candidate-a', 'pet-adopted', 'a.png', 'ok', 0, '20'),
                    ('candidate-u', 'pet-upload', 'u.png', 'ok', 1, '30')",
            [],
        )
        .unwrap();

        apply(&db).unwrap();

        let rows = ["pet-guided", "pet-adopted", "pet-upload"].map(|pet_id| {
            db.query_row(
                "SELECT p.creation_method, cs.status
                     FROM pets p JOIN creation_sessions cs ON cs.pet_id=p.pet_id
                     WHERE p.pet_id=?1",
                [pet_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .unwrap()
        });
        assert_eq!(rows[0], ("composer".into(), "draft".into()));
        assert_eq!(rows[1], ("adoption".into(), "candidateReady".into()));
        assert_eq!(rows[2], ("upload".into(), "completed".into()));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn migration_marks_older_long_lived_drafts_before_creating_unique_index() {
        let (db, root) = temp_db();
        db.execute_batch(MIGRATIONS[0]).unwrap();
        db.execute_batch(MIGRATIONS[1]).unwrap();
        db.pragma_update(None, "user_version", 2).unwrap();
        insert_v2_pet(&db, "pet-older", "realpet", "10");
        insert_v2_pet(&db, "pet-newer", "guided", "20");
        db.execute(
            "INSERT INTO generation_jobs
             (job_id, pet_id, prompt, ref_sha256, status, created_at)
             VALUES ('job-old', 'pet-older', 'p', 'h', 'pending', '10'),
                    ('job-new', 'pet-newer', 'p', 'h', 'pending', '20')",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO appearance_variants
             (variant_id, pet_id, job_id, image_path, cutout_path, quality, accepted, created_at)
             VALUES ('candidate-old', 'pet-older', 'job-old', 'raw.png',
                     'C:\\jobs\\job-old\\cutout.png', 'ok', 0, '10'),
                    ('candidate-new', 'pet-newer', 'job-new', 'raw.png',
                     'C:\\jobs\\job-new\\cutout.png', 'ok', 0, '20')",
            [],
        )
        .unwrap();

        apply(&db).unwrap();

        let older: (String, String) = db
            .query_row(
                "SELECT p.lifecycle, cs.status FROM pets p
                 JOIN creation_sessions cs ON cs.pet_id=p.pet_id
                 WHERE p.pet_id='pet-older'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(older, ("abandoned".into(), "abandoned".into()));
        let newer_status: String = db
            .query_row(
                "SELECT status FROM creation_sessions WHERE pet_id='pet-newer'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(newer_status, "candidateReady");

        for (job_id, expected_session) in [
            ("job-old", "session-migrated-pet-older"),
            ("job-new", "session-migrated-pet-newer"),
        ] {
            let actual: String = db
                .query_row(
                    "SELECT session_id FROM generation_jobs WHERE job_id=?1",
                    [job_id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(actual, expected_session);
        }
        let candidate: (String, String) = db
            .query_row(
                "SELECT session_id, motion_profile_path FROM appearance_variants
                 WHERE variant_id='candidate-old'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(candidate.0, "session-migrated-pet-older");
        assert_eq!(candidate.1, "C:\\jobs\\job-old\\motion-profile.json");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn database_rejects_two_long_lived_drafts() {
        let (db, root) = temp_db();
        apply(&db).unwrap();
        insert_draft_pet_and_session(&db, "pet-a", "session-a", "upload").unwrap();
        let error =
            insert_draft_pet_and_session(&db, "pet-b", "session-b", "composer").unwrap_err();
        assert!(error.to_string().contains("UNIQUE"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn database_rejects_duplicate_adoption_sources() {
        let (db, root) = temp_db();
        apply(&db).unwrap();
        for pet_id in ["pet-a", "pet-b"] {
            let result = db.execute(
                "INSERT INTO pets
                 (pet_id, schema_version, species, identity_mode, creation_method,
                  source_template_id, source_template_version, lifecycle, created_at, updated_at)
                 VALUES (?1, 1, 'cat', 'adopted', 'adoption', 'template-a', 1,
                         'draft', '10', '10')",
                [pet_id],
            );
            if pet_id == "pet-a" {
                result.unwrap();
            } else {
                assert!(result.unwrap_err().to_string().contains("UNIQUE"));
            }
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn database_rejects_invalid_source_template_inserts() {
        let (db, root) = temp_db();
        apply(&db).unwrap();
        let invalid_rows = [
            ("pet-id-only", "adoption", Some("template-a"), None),
            ("pet-version-only", "adoption", None, Some(1)),
            ("pet-upload-source", "upload", Some("template-b"), Some(1)),
            (
                "pet-composer-source",
                "composer",
                Some("template-c"),
                Some(1),
            ),
        ];

        for (pet_id, method, template_id, template_version) in invalid_rows {
            let error = db
                .execute(
                    "INSERT INTO pets
                     (pet_id, schema_version, species, identity_mode, creation_method,
                      source_template_id, source_template_version, lifecycle,
                      created_at, updated_at)
                     VALUES (?1, 1, 'cat', 'realpet', ?2, ?3, ?4, 'draft', '10', '10')",
                    rusqlite::params![pet_id, method, template_id, template_version],
                )
                .unwrap_err();
            assert!(error.to_string().contains("source template"));
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn database_rejects_invalid_source_template_updates() {
        let (db, root) = temp_db();
        apply(&db).unwrap();
        db.execute(
            "INSERT INTO pets
             (pet_id, schema_version, species, identity_mode, creation_method,
              source_template_id, source_template_version, lifecycle, created_at, updated_at)
             VALUES ('pet-a', 1, 'cat', 'adopted', 'adoption',
                     'template-a', 1, 'draft', '10', '10')",
            [],
        )
        .unwrap();

        for sql in [
            "UPDATE pets SET source_template_version=NULL WHERE pet_id='pet-a'",
            "UPDATE pets SET creation_method='upload' WHERE pet_id='pet-a'",
        ] {
            let error = db.execute(sql, []).unwrap_err();
            assert!(error.to_string().contains("source template"));
        }

        db.execute(
            "UPDATE pets
             SET source_template_id=NULL, source_template_version=NULL
             WHERE pet_id='pet-a'",
            [],
        )
        .unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn database_accepts_all_supported_creation_methods() {
        let (db, root) = temp_db();
        apply(&db).unwrap();
        for method in ["upload", "composer", "adoption"] {
            db.execute(
                "INSERT INTO pets
                 (pet_id, schema_version, species, identity_mode, creation_method,
                  lifecycle, created_at, updated_at)
                 VALUES (?1, 1, 'cat', 'realpet', ?1, 'draft', '10', '10')",
                [method],
            )
            .unwrap();
        }
        let count: i64 = db
            .query_row("SELECT COUNT(*) FROM pets", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 3);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn database_rejects_invalid_creation_method_insert() {
        let (db, root) = temp_db();
        apply(&db).unwrap();
        let error = db
            .execute(
                "INSERT INTO pets
                 (pet_id, schema_version, species, identity_mode, creation_method,
                  lifecycle, created_at, updated_at)
                 VALUES ('pet-a', 1, 'cat', 'realpet', 'invalid', 'draft', '10', '10')",
                [],
            )
            .unwrap_err();
        assert!(error.to_string().contains("CHECK constraint failed"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn database_rejects_invalid_creation_method_update() {
        let (db, root) = temp_db();
        apply(&db).unwrap();
        db.execute(
            "INSERT INTO pets
             (pet_id, schema_version, species, identity_mode, creation_method,
              lifecycle, created_at, updated_at)
             VALUES ('pet-a', 1, 'cat', 'realpet', 'upload', 'draft', '10', '10')",
            [],
        )
        .unwrap();
        let error = db
            .execute(
                "UPDATE pets SET creation_method='invalid' WHERE pet_id='pet-a'",
                [],
            )
            .unwrap_err();
        assert!(error.to_string().contains("CHECK constraint failed"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn v9_adds_photo_avatar_tables_without_rewriting_legacy_upload_rows() {
        let (db, root) = temp_db();
        apply_through(&db, 8);
        db.execute(
            "INSERT INTO pets
             (pet_id, schema_version, species, identity_mode, creation_method,
              lifecycle, created_at, updated_at)
             VALUES ('pet-legacy', 1, 'cat', 'realpet', 'upload', 'draft', '10', '10')",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO creation_sessions
             (session_id, pet_id, method, status, last_stable_status, current_step,
              schema_version, created_at, updated_at)
             VALUES ('session-legacy', 'pet-legacy', 'upload', 'draft', 'draft', 'upload',
                     1, '10', '10')",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO creation_upload_sources
             (session_id, normalized_png, sha256, mime_type, byte_size, created_at)
             VALUES ('session-legacy', X'00',
                     '0000000000000000000000000000000000000000000000000000000000000000',
                     'image/png', 1, '10')",
            [],
        )
        .unwrap();

        db.execute_batch(MIGRATIONS[8]).unwrap();
        db.pragma_update(None, "user_version", 9).unwrap();

        let version: i64 = db
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 9);
        let legacy_count: i64 = db
            .query_row("SELECT COUNT(*) FROM creation_upload_sources", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(legacy_count, 1);
        for table in [
            "photo_avatar_consents",
            "photo_avatar_sources",
            "photo_avatar_runs",
            "photo_avatar_step_attempts",
            "photo_avatar_profiles",
            "photo_avatar_artifacts",
        ] {
            assert!(table_exists(&db, table).unwrap(), "{table}");
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn v9_rejects_current_version_without_photo_avatar_schema() {
        let (db, root) = temp_db();
        apply_through(&db, 8);
        db.pragma_update(None, "user_version", 9).unwrap();

        let error = apply(&db).unwrap_err();

        assert!(error.contains("photo_avatar_consents"), "{error}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn v10_preserves_v1_consent_and_accepts_explicit_lk888_v2_disclosure() {
        let (db, root) = temp_db();
        apply_through(&db, 9);
        db.execute(
            "INSERT INTO photo_avatar_consents(consent_version, accepted_at)
             VALUES ('photo-avatar-third-party-ai-v1', '10')",
            [],
        )
        .unwrap();

        apply(&db).unwrap();

        assert_eq!(
            db.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            MIGRATIONS.len() as i64
        );
        db.execute(
            "INSERT INTO photo_avatar_consents
             (consent_version, provider_id, disclosure_sha256, accepted_at)
             VALUES (?1, 'lk888', ?2, '11')",
            rusqlite::params![
                "photo-avatar-third-party-ai-lk888-no-delete-v2",
                "fa6ad319cea369bb51349b9b16d11544ecab71ba0bbb027c32b624f72c86a3be"
            ],
        )
        .unwrap();
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM photo_avatar_consents", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            2
        );
        let legacy: (Option<String>, Option<String>) = db
            .query_row(
                "SELECT provider_id, disclosure_sha256 FROM photo_avatar_consents
                 WHERE consent_version='photo-avatar-third-party-ai-v1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(legacy, (None, None));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn v10_rejects_noncanonical_consent_and_cleanup_audit_states() {
        let (db, root) = temp_db();
        apply(&db).unwrap();
        for values in [
            (
                "other",
                "fa6ad319cea369bb51349b9b16d11544ecab71ba0bbb027c32b624f72c86a3be",
            ),
            (
                "lk888",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ),
        ] {
            assert!(db
                .execute(
                    "INSERT INTO photo_avatar_consents
                     (consent_version, provider_id, disclosure_sha256, accepted_at)
                     VALUES ('photo-avatar-third-party-ai-lk888-no-delete-v2', ?1, ?2, '10')",
                    rusqlite::params![values.0, values.1],
                )
                .is_err());
        }
        assert!(db
            .execute(
                "INSERT INTO photo_avatar_cleanup_audit
                 (session_id, revision, local_cleanup, backend_cleanup, upstream_cleanup,
                  provider_id, updated_at)
                 VALUES ('session', 1, 'deleted', 'deleted', 'deleted', 'lk888', '10')",
                [],
            )
            .is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn v11_adds_photo_avatar_artifact_audit_without_rewriting_existing_rows() {
        let (db, root) = temp_db();
        apply_through(&db, 10);
        insert_draft_pet_and_session(&db, "pet-legacy", "session-legacy", "upload").unwrap();
        db.execute(
            "INSERT INTO photo_avatar_runs
             (session_id, revision, step, generation_token, updated_at)
             VALUES ('session-legacy', 1, 'buildV5', 'token-legacy', '10')",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO photo_avatar_artifacts
             (session_id, revision, kind, relative_path, sha256, local_path, created_at)
             VALUES ('session-legacy', 1, 'textureAtlas', 'atlas.png', ?1, NULL, '10')",
            ["00".repeat(32)],
        )
        .unwrap();

        apply(&db).unwrap();

        assert_eq!(
            db.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            MIGRATIONS.len() as i64
        );
        let row: (String, Option<String>) = db
            .query_row(
                "SELECT relative_path, audit_json FROM photo_avatar_artifacts
                 WHERE session_id='session-legacy' AND revision=1 AND kind='textureAtlas'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(row, ("atlas.png".into(), None));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn v13_backfills_pixel_and_live2d_run_styles() {
        let (db, root) = temp_db();
        apply_through(&db, 12);
        insert_draft_pet_and_session(&db, "pet-pixel", "session-pixel", "upload").unwrap();
        db.execute(
            "UPDATE creation_sessions SET status='completed' WHERE session_id='session-pixel'",
            [],
        )
        .unwrap();
        insert_draft_pet_and_session(&db, "pet-live2d", "session-live2d", "upload").unwrap();
        db.execute(
            "INSERT INTO photo_avatar_runs
             (session_id, revision, route, step, generation_token, updated_at)
             VALUES ('session-pixel', 1, 'pixel-v1', 'analyzeIdentity', 'token-pixel', '10')",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO photo_avatar_runs
             (session_id, revision, route, step, generation_token, updated_at)
             VALUES ('session-live2d', 1, 'live2d-v5', 'analyzeIdentity', 'token-live2d', '10')",
            [],
        )
        .unwrap();

        apply(&db).unwrap();

        let pixel: String = db
            .query_row(
                "SELECT style_profile_id FROM photo_avatar_runs WHERE session_id='session-pixel'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let live2d: String = db
            .query_row(
                "SELECT style_profile_id FROM photo_avatar_runs WHERE session_id='session-live2d'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pixel, "pixel-style-v1");
        assert_eq!(live2d, "live2d-v5");
        let _ = std::fs::remove_dir_all(root);
    }
}
