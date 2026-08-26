//! Diesel schema, generated from the SQL migrations in `migrations/`.
//!
//! Regenerate after every migration with `just schema`
//! (`DATABASE_URL=<scratch db> diesel print-schema > src/schema.rs`),
//! then widen `Integer` → `BigInt` and `Float` → `Double`: SQLite's
//! dynamic typing makes both valid, and 64-bit widths match the row
//! structs/models without casts at every boundary.

// @generated automatically by Diesel CLI.

diesel::table! {
    book_files (id) {
        id -> Text,
        book_id -> Text,
        source -> Text,
        external_id -> Text,
        content_source -> Nullable<Text>,
        content_external_id -> Nullable<Text>,
        format -> Text,
        label -> Text,
        visibility -> Text,
        owner_id -> Nullable<Text>,
        chapter_count -> BigInt,
        toc -> Text,
        created_at -> Text,
        volume_no -> BigInt,
        volume_offset -> BigInt,
        orig_ext -> Nullable<Text>,
        orig_sha256 -> Nullable<Text>,
        orig_size -> Nullable<BigInt>,
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
        created_at -> Text,
        cover -> Nullable<Binary>,
        cover_mime -> Nullable<Text>,
        series_id -> Nullable<Text>,
        volume_no -> BigInt,
    }
}

diesel::table! {
    chapters (file_id, idx) {
        file_id -> Text,
        idx -> BigInt,
        title -> Text,
        content -> Text,
    }
}

diesel::table! {
    images (id) {
        id -> Text,
        mime -> Text,
        size -> BigInt,
        created_at -> Text,
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
        created_at -> Text,
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
        created_at -> Text,
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
        updated_at -> Text,
    }
}

diesel::table! {
    shares (token) {
        token -> Text,
        kind -> Text,
        mode -> Text,
        file_id -> Text,
        session_id -> Nullable<Text>,
        created_by -> Nullable<Text>,
        expires_at -> Nullable<Text>,
        created_at -> Text,
    }
}

diesel::table! {
    users (id) {
        id -> Text,
        username -> Text,
        password_hash -> Text,
        role -> Text,
        created_at -> Text,
    }
}

diesel::joinable!(book_files -> books (book_id));
diesel::joinable!(book_files -> users (owner_id));
diesel::joinable!(books -> series (series_id));
diesel::joinable!(books -> users (created_by));
diesel::joinable!(chapters -> book_files (file_id));
diesel::joinable!(series -> users (created_by));
diesel::joinable!(sessions -> book_files (file_id));
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
    series,
    sessions,
    shares,
    users,
);
