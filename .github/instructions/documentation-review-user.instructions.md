---
description: "Documentation review guidelines for user-facing docs"
applyTo: "docs/**/*.md"
exclude: "docs/**/contributing/*.md"
---

# Documentation Review Guidelines (User-Facing)

## Audience

Documentation outside of `contributing/` is for **end users** who may have no technical background.
Assume the reader is using Cadmus on their Kobo e-reader and needs clear, simple instructions.

## Tone and Language

- **Conversational and friendly** - Write like you're explaining to a friend
- **No technical jargon** - Avoid terms like "artifact", "deploy", "CI/CD", "configure", "bundle"
- **Simple alternatives**:
  - "artifact" → "file" or "package"
  - "deploy" → "install" or "download"
  - "configure" → "add" or "set up"
  - "bundle" → "package"
  - "on-device" → "on your Kobo" or "wirelessly"
- **Active voice** - "Copy the file" not "The file should be copied"

## Clarity and Structure

- Use short paragraphs (2-3 sentences max)
- Bullet lists for steps and options
- Clear headings that describe actions ("How to update" not "Update procedure")
- Explain **why** something matters when it's not obvious
- Remove redundant or filler text

## Formatting

- Format Markdown with Prettier
- Ensure Markdown passes markdownlint
