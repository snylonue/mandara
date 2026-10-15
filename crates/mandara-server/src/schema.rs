//! Diesel schema generated with `just schema`.
//!
//! The Diesel CLI applies `schema.patch` from `diesel.toml` to preserve
//! domain enum mappings, native UTC timestamps, 64-bit numbers, and the
//! escaped `offset` column. See `docs/database-types.md` for regeneration.

// @generated automatically by Diesel CLI.

diesel::table! {
    book_files (id) {
        id -> Text,
        book_id -> Text,
        source -> Text,
        external_id -> Text,
        format -> mandara_core::model::FileFormatMapping,
        label -> Text,
        visibility -> mandara_core::model::VisibilityMapping,
        owner_id -> Nullable<Text>,
        chapter_count -> BigInt,
        toc -> Text,
        created_at -> diesel::sql_types::TimestamptzSqlite,
        orig_ext -> Nullable<Text>,
        orig_sha256 -> Nullable<Text>,
        orig_size -> Nullable<BigInt>,
        content_mode -> mandara_core::model::ContentModeMapping,
    }
}

diesel::table! {
    books (id) {
        id -> Text,
        title -> Text,
        authors -> Text,
        description -> Nullable<Text>,
        cover_url -> Nullable<Text>,
        created_by -> Nullable<Text>,
        created_at -> diesel::sql_types::TimestamptzSqlite,
        cover -> Nullable<Binary>,
        cover_mime -> Nullable<Text>,
        series_id -> Nullable<Text>,
        volume_no -> BigInt,
        ext -> Text,
        meta_source -> Nullable<Text>,
        meta_external_id -> Nullable<Text>,
    }
}

diesel::table! {
    chapters (file_id, idx) {
        file_id -> Text,
        idx -> BigInt,
        title -> Text,
        content -> Nullable<Text>,
        linear -> Bool,
    }
}

diesel::table! {
    images (id) {
        id -> Text,
        mime -> Text,
        size -> BigInt,
        created_at -> diesel::sql_types::TimestamptzSqlite,
        width -> Nullable<BigInt>,
        height -> Nullable<BigInt>,
    }
}

diesel::table! {
    plugin_instances (id) {
        id -> Text,
        wasm_file -> Text,
        config -> Text,
        enabled -> BigInt,
        created_at -> diesel::sql_types::TimestamptzSqlite,
    }
}

diesel::table! {
    plugin_state (instance_id, key) {
        instance_id -> Text,
        key -> Text,
        value -> Binary,
    }
}

diesel::table! {
    reading_sessions (id) {
        id -> Text,
        user_id -> Text,
        label -> Text,
        created_at -> diesel::sql_types::TimestamptzSqlite,
    }
}

diesel::table! {
    series (id) {
        id -> Text,
        title -> Text,
        authors -> Text,
        description -> Nullable<Text>,
        cover_url -> Nullable<Text>,
        created_by -> Nullable<Text>,
        created_at -> diesel::sql_types::TimestamptzSqlite,
        ext -> Text,
        meta_source -> Nullable<Text>,
        meta_external_id -> Nullable<Text>,
    }
}

diesel::table! {
    sessions (id) {
        id -> Text,
        user_id -> Text,
        file_id -> Text,
        label -> Text,
        chapter_idx -> BigInt,
        /// `offset` is a Diesel keyword; the column keeps its SQL name.
        #[sql_name = "offset"]
        offset -> BigInt,
        fraction -> Double,
        updated_at -> diesel::sql_types::TimestamptzSqlite,
        reading_session_id -> Nullable<Text>,
        position_kind -> mandara_core::model::PositionKindMapping,
    }
}

diesel::table! {
    shares (token) {
        token -> Text,
        kind -> mandara_core::model::ShareKindMapping,
        file_id -> Text,
        session_id -> Nullable<Text>,
        created_by -> Nullable<Text>,
        expires_at -> Nullable<diesel::sql_types::TimestamptzSqlite>,
        created_at -> diesel::sql_types::TimestamptzSqlite,
    }
}

diesel::table! {
    users (id) {
        id -> Text,
        username -> Text,
        password_hash -> Text,
        role -> mandara_core::model::RoleMapping,
        created_at -> diesel::sql_types::TimestamptzSqlite,
    }
}

diesel::joinable!(book_files -> books (book_id));
diesel::joinable!(book_files -> users (owner_id));
diesel::joinable!(books -> series (series_id));
diesel::joinable!(books -> users (created_by));
diesel::joinable!(chapters -> book_files (file_id));
diesel::joinable!(plugin_state -> plugin_instances (instance_id));
diesel::joinable!(reading_sessions -> users (user_id));
diesel::joinable!(series -> users (created_by));
diesel::joinable!(sessions -> book_files (file_id));
diesel::joinable!(sessions -> reading_sessions (reading_session_id));
diesel::joinable!(sessions -> users (user_id));
diesel::joinable!(shares -> book_files (file_id));
diesel::joinable!(shares -> sessions (session_id));
diesel::joinable!(shares -> users (created_by));

diesel::allow_tables_to_appear_in_same_query!(
    book_files,
    books,
    chapters,
    images,
    plugin_instances,
    plugin_state,
    reading_sessions,
    series,
    sessions,
    shares,
    users,
);
