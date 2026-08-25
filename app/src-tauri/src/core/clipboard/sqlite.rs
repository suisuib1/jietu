use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
    time::Duration,
};

use rusqlite::{Connection, OptionalExtension, Row, Transaction, params, types::Type};

use crate::core::image::png_bytes;

use super::{
    ClipboardImagePreview, ClipboardInput, ClipboardItem, ClipboardKind, ClipboardStorage,
    PrivacyPolicy, RecordOutcome, RetentionPolicy, RetentionResult, StorageError, content_hash,
};

const SCHEMA_VERSION: i64 = 1;
const DATABASE_FILE: &str = "clipboard.db";
const IMAGE_DIRECTORY: &str = "ClipboardImages";
const MAX_QUERY_RESULTS: usize = 500;

pub(crate) struct SqliteClipboardStorage {
    connection: Mutex<Connection>,
    database_path: PathBuf,
    image_directory: PathBuf,
    privacy: PrivacyPolicy,
    retention: RetentionPolicy,
}

impl SqliteClipboardStorage {
    pub(crate) fn open(
        data_directory: impl AsRef<Path>,
        privacy: PrivacyPolicy,
        retention: RetentionPolicy,
    ) -> Result<Self, StorageError> {
        let data_directory = data_directory.as_ref();
        fs::create_dir_all(data_directory)?;
        let image_directory = data_directory.join(IMAGE_DIRECTORY);
        fs::create_dir_all(&image_directory)?;
        let database_path = data_directory.join(DATABASE_FILE);
        let mut connection = Connection::open(&database_path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        migrate(&mut connection)?;

        Ok(Self {
            connection: Mutex::new(connection),
            database_path,
            image_directory,
            privacy,
            retention,
        })
    }

    pub(crate) fn database_path(&self) -> &Path {
        &self.database_path
    }

    pub(crate) fn image_directory(&self) -> &Path {
        &self.image_directory
    }

    fn lock_connection(&self) -> Result<MutexGuard<'_, Connection>, StorageError> {
        self.connection
            .lock()
            .map_err(|_| StorageError::LockPoisoned)
    }

    fn find_by_hash(
        &self,
        connection: &Connection,
        hash: &str,
    ) -> Result<Option<ClipboardItem>, StorageError> {
        connection
            .query_row(
                "SELECT id, kind, text_content, html_content, image_file, files_json, \
                 content_hash, source_app, created_at_ms, last_used_at_ms, favorite \
                 FROM clipboard_items WHERE content_hash = ?1",
                [hash],
                |row| item_from_row(row, &self.image_directory),
            )
            .optional()
            .map_err(StorageError::from)
    }

    fn write_image(
        &self,
        hash: &str,
        width: u32,
        height: u32,
        rgba8: &[u8],
    ) -> Result<(String, bool), StorageError> {
        let file_name = format!("{hash}.png");
        let destination = self.image_directory.join(&file_name);
        if destination.exists() {
            return Ok((file_name, false));
        }

        let image = image::RgbaImage::from_raw(width, height, rgba8.to_vec())
            .ok_or_else(|| StorageError::InvalidData("invalid RGBA8 image buffer".into()))?;
        let bytes = png_bytes(&image).map_err(StorageError::InvalidData)?;
        let temporary = self.image_directory.join(format!(".{hash}.tmp"));
        if temporary.exists() {
            fs::remove_file(&temporary)?;
        }
        fs::write(&temporary, bytes)?;
        if let Err(error) = fs::rename(&temporary, &destination) {
            let _ = fs::remove_file(&temporary);
            return Err(StorageError::Io(error));
        }
        Ok((file_name, true))
    }

    fn remove_unreferenced_images(
        &self,
        connection: &Connection,
        image_files: impl IntoIterator<Item = String>,
    ) -> Result<usize, StorageError> {
        let mut deleted = 0;
        for image_file in image_files {
            if !is_managed_image_file(&image_file) {
                continue;
            }
            let references: i64 = connection.query_row(
                "SELECT COUNT(*) FROM clipboard_items WHERE image_file = ?1",
                [&image_file],
                |row| row.get(0),
            )?;
            if references == 0 {
                let path = self.image_directory.join(image_file);
                match fs::remove_file(path) {
                    Ok(()) => deleted += 1,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(StorageError::Io(error)),
                }
            }
        }
        Ok(deleted)
    }
}

impl ClipboardStorage for SqliteClipboardStorage {
    fn record(&self, input: ClipboardInput, now_ms: i64) -> Result<RecordOutcome, StorageError> {
        if let Err(rejection) = self.privacy.validate(&input) {
            return Ok(RecordOutcome::Ignored(rejection));
        }

        let hash = content_hash(&input);
        let mut connection = self.lock_connection()?;
        if self.find_by_hash(&connection, &hash)?.is_some() {
            connection.execute(
                "UPDATE clipboard_items \
                 SET last_used_at_ms = MAX(last_used_at_ms, ?1), \
                     source_app = COALESCE(?2, source_app) \
                 WHERE content_hash = ?3",
                params![now_ms, input.source_app(), hash],
            )?;
            let item = self
                .find_by_hash(&connection, &hash)?
                .ok_or_else(|| StorageError::InvalidData("deduplicated item disappeared".into()))?;
            return Ok(RecordOutcome::Duplicate(item));
        }

        let transaction = connection.transaction()?;
        let mut created_image = false;
        let (text_content, html_content, image_file, files) = match &input {
            ClipboardInput::Text { text, .. } => (Some(text.clone()), None, None, Vec::new()),
            ClipboardInput::Html { html, text, .. } => {
                (text.clone(), Some(html.clone()), None, Vec::new())
            }
            ClipboardInput::Image {
                width,
                height,
                rgba8,
                ..
            } => {
                let (file, created) = self.write_image(&hash, *width, *height, rgba8)?;
                created_image = created;
                (None, None, Some(file), Vec::new())
            }
            ClipboardInput::Files { files, .. } => (None, None, None, files.clone()),
        };
        let files_json = serde_json::to_string(&files)
            .map_err(|error| StorageError::InvalidData(error.to_string()))?;

        let insert_result = transaction.execute(
            "INSERT INTO clipboard_items (kind, text_content, html_content, image_file, \
             files_json, content_hash, source_app, created_at_ms, last_used_at_ms, favorite) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, 0)",
            params![
                input.kind().as_str(),
                text_content,
                html_content,
                image_file,
                files_json,
                hash,
                input.source_app(),
                now_ms,
            ],
        );
        if let Err(error) = insert_result {
            if created_image {
                let _ = fs::remove_file(self.image_directory.join(format!("{hash}.png")));
            }
            return Err(StorageError::Database(error));
        }
        if let Err(error) = transaction.commit() {
            if created_image {
                let _ = fs::remove_file(self.image_directory.join(format!("{hash}.png")));
            }
            return Err(StorageError::Database(error));
        }

        let item = self
            .find_by_hash(&connection, &hash)?
            .ok_or_else(|| StorageError::InvalidData("inserted item disappeared".into()))?;
        Ok(RecordOutcome::Inserted(item))
    }

    fn get(&self, id: i64) -> Result<Option<ClipboardItem>, StorageError> {
        let connection = self.lock_connection()?;
        connection
            .query_row(
                "SELECT id, kind, text_content, html_content, image_file, files_json, \
                 content_hash, source_app, created_at_ms, last_used_at_ms, favorite \
                 FROM clipboard_items WHERE id = ?1",
                [id],
                |row| item_from_row(row, &self.image_directory),
            )
            .optional()
            .map_err(StorageError::from)
    }

    fn list_page(&self, offset: usize, limit: usize) -> Result<Vec<ClipboardItem>, StorageError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let connection = self.lock_connection()?;
        query_items(
            &connection,
            "SELECT id, kind, text_content, html_content, image_file, files_json, \
             content_hash, source_app, created_at_ms, last_used_at_ms, favorite \
             FROM clipboard_items ORDER BY last_used_at_ms DESC, id DESC LIMIT ?1 OFFSET ?2",
            [normalized_limit(limit) as i64, normalized_offset(offset)],
            &self.image_directory,
        )
    }

    fn search_page(
        &self,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<ClipboardItem>, StorageError> {
        let query = query.trim();
        if query.is_empty() {
            return self.list_page(offset, limit);
        }
        if limit == 0 {
            return Ok(Vec::new());
        }
        let pattern = format!("%{}%", escape_like(query));
        let connection = self.lock_connection()?;
        let mut statement = connection.prepare(
            "SELECT id, kind, text_content, html_content, image_file, files_json, \
             content_hash, source_app, created_at_ms, last_used_at_ms, favorite \
             FROM clipboard_items \
             WHERE text_content LIKE ?1 ESCAPE '\\' COLLATE NOCASE \
                OR source_app LIKE ?1 ESCAPE '\\' COLLATE NOCASE \
                OR files_json LIKE ?1 ESCAPE '\\' COLLATE NOCASE \
              ORDER BY last_used_at_ms DESC, id DESC LIMIT ?2 OFFSET ?3",
        )?;
        let items = statement
            .query_map(
                params![
                    pattern,
                    normalized_limit(limit) as i64,
                    normalized_offset(offset)
                ],
                |row| item_from_row(row, &self.image_directory),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(items)
    }

    fn count(&self, query: Option<&str>) -> Result<usize, StorageError> {
        let query = query.map(str::trim).filter(|value| !value.is_empty());
        let connection = self.lock_connection()?;
        let count: i64 = if let Some(query) = query {
            let pattern = format!("%{}%", escape_like(query));
            connection.query_row(
                "SELECT COUNT(*) FROM clipboard_items \
                 WHERE text_content LIKE ?1 ESCAPE '\\' COLLATE NOCASE \
                    OR source_app LIKE ?1 ESCAPE '\\' COLLATE NOCASE \
                    OR files_json LIKE ?1 ESCAPE '\\' COLLATE NOCASE",
                [pattern],
                |row| row.get(0),
            )?
        } else {
            connection.query_row("SELECT COUNT(*) FROM clipboard_items", [], |row| row.get(0))?
        };
        usize::try_from(count)
            .map_err(|_| StorageError::InvalidData("clipboard item count is invalid".into()))
    }

    fn delete(&self, id: i64) -> Result<bool, StorageError> {
        let mut connection = self.lock_connection()?;
        let transaction = connection.transaction()?;
        let image_file = transaction
            .query_row(
                "SELECT image_file FROM clipboard_items WHERE id = ?1",
                [id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?;
        if image_file.is_none() {
            return Ok(false);
        }
        transaction.execute("DELETE FROM clipboard_items WHERE id = ?1", [id])?;
        transaction.commit()?;
        if let Some(image_file) = image_file.flatten() {
            self.remove_unreferenced_images(&connection, [image_file])?;
        }
        Ok(true)
    }

    fn set_favorite(&self, id: i64, favorite: bool) -> Result<bool, StorageError> {
        let connection = self.lock_connection()?;
        Ok(connection.execute(
            "UPDATE clipboard_items SET favorite = ?1 WHERE id = ?2",
            params![favorite, id],
        )? > 0)
    }

    fn image_preview(
        &self,
        id: i64,
        max_width: u32,
        max_height: u32,
    ) -> Result<Option<ClipboardImagePreview>, StorageError> {
        if max_width == 0 || max_height == 0 {
            return Err(StorageError::InvalidData(
                "image preview dimensions must be positive".into(),
            ));
        }
        let Some(item) = self.get(id)? else {
            return Ok(None);
        };
        if item.kind != ClipboardKind::Image {
            return Err(StorageError::InvalidData(
                "clipboard item is not an image".into(),
            ));
        }
        let path = item
            .image_path
            .ok_or_else(|| StorageError::InvalidData("image item has no managed file".into()))?;
        let file_name = path
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| is_managed_image_file(value))
            .ok_or_else(|| StorageError::InvalidData("invalid managed image path".into()))?;
        if path != self.image_directory.join(file_name) {
            return Err(StorageError::InvalidData(
                "managed image path escaped its storage directory".into(),
            ));
        }
        if !path.is_file() {
            return Ok(None);
        }

        let source = image::open(&path)
            .map_err(|error| StorageError::InvalidData(format!("unable to decode image: {error}")))?
            .into_rgba8();
        let source_width = source.width().max(1);
        let source_height = source.height().max(1);
        let scale = (f64::from(max_width) / f64::from(source_width))
            .min(f64::from(max_height) / f64::from(source_height))
            .min(1.0);
        let preview_width = (f64::from(source_width) * scale).round().max(1.0) as u32;
        let preview_height = (f64::from(source_height) * scale).round().max(1.0) as u32;
        let preview = image::imageops::resize(
            &source,
            preview_width,
            preview_height,
            image::imageops::FilterType::Lanczos3,
        );
        let width = preview.width();
        let height = preview.height();
        let png = png_bytes(&preview).map_err(StorageError::InvalidData)?;
        Ok(Some(ClipboardImagePreview { png, width, height }))
    }

    fn enforce_retention(&self, now_ms: i64) -> Result<RetentionResult, StorageError> {
        let mut connection = self.lock_connection()?;
        let transaction = connection.transaction()?;
        let mut removals = BTreeMap::<i64, Option<String>>::new();

        if let Some(max_age) = self.retention.max_age {
            let cutoff = now_ms.saturating_sub(duration_millis(max_age));
            collect_removals(
                &transaction,
                "SELECT id, image_file FROM clipboard_items \
                 WHERE favorite = 0 AND last_used_at_ms < ?1",
                [cutoff],
                &mut removals,
            )?;
        }

        let favorite_count: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM clipboard_items WHERE favorite = 1",
            [],
            |row| row.get(0),
        )?;
        let regular_limit = self
            .retention
            .max_items
            .saturating_sub(usize::try_from(favorite_count).unwrap_or(usize::MAX));
        let mut statement = transaction.prepare(
            "SELECT id, image_file FROM clipboard_items \
             WHERE favorite = 0 ORDER BY last_used_at_ms DESC, id DESC LIMIT -1 OFFSET ?1",
        )?;
        for result in statement.query_map([regular_limit as i64], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
        })? {
            let (id, image_file) = result?;
            removals.insert(id, image_file);
        }
        drop(statement);

        for id in removals.keys() {
            transaction.execute("DELETE FROM clipboard_items WHERE id = ?1", [id])?;
        }
        transaction.commit()?;

        let deleted_items = removals.len();
        let image_files = removals.into_values().flatten().collect::<Vec<_>>();
        let deleted_images = self.remove_unreferenced_images(&connection, image_files)?;
        Ok(RetentionResult {
            deleted_items,
            deleted_images,
        })
    }
}

fn migrate(connection: &mut Connection) -> Result<(), StorageError> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version > SCHEMA_VERSION {
        return Err(StorageError::UnsupportedSchema(version));
    }
    if version == 0 {
        let transaction = connection.transaction()?;
        transaction.execute_batch(
            "CREATE TABLE clipboard_items (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                kind TEXT NOT NULL CHECK (kind IN ('text', 'html', 'image', 'files')),
                text_content TEXT,
                html_content TEXT,
                image_file TEXT,
                files_json TEXT NOT NULL DEFAULT '[]',
                content_hash TEXT NOT NULL UNIQUE CHECK (length(content_hash) = 64),
                source_app TEXT,
                created_at_ms INTEGER NOT NULL,
                last_used_at_ms INTEGER NOT NULL,
                favorite INTEGER NOT NULL DEFAULT 0 CHECK (favorite IN (0, 1))
            );
            CREATE INDEX clipboard_items_last_used_idx
                ON clipboard_items(last_used_at_ms DESC);
            CREATE INDEX clipboard_items_favorite_idx
                ON clipboard_items(favorite DESC, last_used_at_ms DESC);
            PRAGMA user_version = 1;",
        )?;
        transaction.commit()?;
    }
    Ok(())
}

fn item_from_row(row: &Row<'_>, image_directory: &Path) -> rusqlite::Result<ClipboardItem> {
    let kind_value: String = row.get(1)?;
    let kind = ClipboardKind::from_str(&kind_value).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            1,
            Type::Text,
            format!("unknown clipboard kind: {kind_value}").into(),
        )
    })?;
    let files_json: String = row.get(5)?;
    let files = serde_json::from_str(&files_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(5, Type::Text, Box::new(error))
    })?;
    let image_file: Option<String> = row.get(4)?;
    if image_file
        .as_deref()
        .is_some_and(|name| !is_managed_image_file(name))
    {
        return Err(rusqlite::Error::FromSqlConversionFailure(
            4,
            Type::Text,
            "invalid managed image file name".into(),
        ));
    }
    Ok(ClipboardItem {
        id: row.get(0)?,
        kind,
        text_content: row.get(2)?,
        html_content: row.get(3)?,
        image_path: image_file.map(|file| image_directory.join(file)),
        files,
        hash: row.get(6)?,
        source_app: row.get(7)?,
        created_at_ms: row.get(8)?,
        last_used_at_ms: row.get(9)?,
        favorite: row.get(10)?,
    })
}

fn query_items<const N: usize>(
    connection: &Connection,
    sql: &str,
    parameters: [i64; N],
    image_directory: &Path,
) -> Result<Vec<ClipboardItem>, StorageError> {
    let mut statement = connection.prepare(sql)?;
    let items = statement
        .query_map(rusqlite::params_from_iter(parameters), |row| {
            item_from_row(row, image_directory)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(items)
}

fn collect_removals<const N: usize>(
    transaction: &Transaction<'_>,
    sql: &str,
    parameters: [i64; N],
    removals: &mut BTreeMap<i64, Option<String>>,
) -> Result<(), StorageError> {
    let mut statement = transaction.prepare(sql)?;
    for result in statement.query_map(rusqlite::params_from_iter(parameters), |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
    })? {
        let (id, image_file) = result?;
        removals.insert(id, image_file);
    }
    Ok(())
}

fn normalized_limit(limit: usize) -> usize {
    limit.min(MAX_QUERY_RESULTS)
}

fn normalized_offset(offset: usize) -> i64 {
    i64::try_from(offset).unwrap_or(i64::MAX)
}

fn duration_millis(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn is_managed_image_file(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 68
        && &bytes[64..] == b".png"
        && bytes[..64]
            .iter()
            .copied()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
