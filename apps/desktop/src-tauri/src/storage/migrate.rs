use rusqlite::Connection;

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
      byte_size INTEGER NOT NULL CHECK(byte_size > 0 AND byte_size <= 25165824),
      created_at TEXT NOT NULL,
      CHECK(length(normalized_png) = byte_size)
    );
    "#,
];

pub fn apply(db: &Connection) -> Result<(), String> {
    let current: i64 = db
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if current as usize >= MIGRATIONS.len() {
        return Ok(());
    }
    for (index, migration) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let version = index + 1;
        let batch =
            format!("BEGIN IMMEDIATE;\n{migration}\nPRAGMA user_version = {version};\nCOMMIT;");
        if let Err(error) = db.execute_batch(&batch) {
            let _ = db.execute_batch("ROLLBACK;");
            return Err(format!("migration {index} failed: {error}"));
        }
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
            sql.contains("byte_size <= 25165824"),
            "missing normalized source hard limit: {sql}"
        );
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
}
