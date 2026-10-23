# UI terminology

This glossary is the shared vocabulary for all zh-CN product surfaces. UI copy lives in `frontend/src/i18n/locales/zh-CN.json`; identifiers and API field names need not be renamed.

| Concept | UI term | Meaning and usage |
| --- | --- | --- |
| Bibliographic record | 书目 | A record that can exist without readable content. Use for creating or deleting the record. |
| Bibliographic fields | 书目信息 | Title, authors, description and extended fields. Do not use 元数据 in product UI. |
| Readable resource | 书籍内容 | An uploaded file or plugin-provided resource attached to a record. Count as 项, not 文件 or 版本. Shorten to 内容 when the context is clear. |
| Actual file | 书籍文件 | An EPUB / TXT file selected for upload or returned by a file provider. File operations may use 文件. |
| Configured provider | 插件源 | A configured plugin instance. Use 书目信息来源 or 内容来源 when explaining its role, not 书源 or 元数据源. Admin configuration may use 插件实例. |
| Provider book identifier | 书籍 ID | An identifier within the selected plugin source, not a local record ID. Show source-specific input hints; do not promise universal URL support. |
| Provider content identifier | 内容 ID | Use only when a source distinguishes content from its bibliographic ID. Prefer selecting enumerated content over constructing opaque IDs. |
| Collection of books | 系列 | A collection whose members are 单卷. Describe collection fields as 系列信息. |
| Edition | 版本 | Reserve for an actual edition, translation or revision. A different resource, source or file format is not automatically a different edition. |
| Chapter text | 正文 | The text inside a readable resource. Do not use as a synonym for the resource itself. |

## Interaction copy

- Getting book information does not imply adding readable content. State both independently before submission.
- Use 仅添加书目 when no content is selected; explain that reading is not yet available.
- Prefer source + ID as the primary acquisition path when lookup is supported. Search is an optional capability, not a universal prerequisite.
- Preserve API error messages; do not infer a cause from an HTTP status alone. Plugin failures that provide no details cannot be diagnosed by frontend copy. Do not expose private server logs to compensate.
