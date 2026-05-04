# OOXML Common Infrastructure

## Overview

This directory contains shared OOXML (Office Open XML) manipulation scripts used by the DOCX and PPTX skills. These scripts provide core functionality for unpacking, modifying, and repacking Office documents (.docx, .pptx, .xlsx).

## Purpose

OOXML is the XML-based file format used by Microsoft Office (Word, PowerPoint, Excel). Office documents are essentially ZIP archives containing XML files and resources. These scripts enable:

- **Unpacking**: Extract ZIP archive and format XML for readability
- **Packing**: Repackage modified XML back into Office format
- **Validation**: Verify document integrity after modifications
- **Safe parsing**: Use defusedxml to prevent XML attacks

## Skills Using OOXML

### DOCX Skill ✓

The DOCX skill uses these scripts via symlink (`docx/ooxml → ../common/ooxml`)

**Use cases**:

- Track changes (insertions/deletions)
- Content modification
- Style management
- Document conversion

### PPTX Skill ✓

The PPTX skill uses these scripts via symlink (`pptx/ooxml → ../common/ooxml`) plus additional PPTX-specific scripts in `pptx/scripts/`

**Use cases**:

- Template-based presentation creation
- Slide rearrangement and duplication
- Text inventory and replacement
- Speaker notes and comments
- Thumbnail generation

## Common OOXML Scripts

### unpack.sh

Unpacks an Office file into a directory with formatted XML.

**Usage**: `bash ooxml/scripts/unpack.sh <office_file> <output_directory>`

**What it does**: Extracts ZIP archive, pretty-prints XML, suggests RSID for tracked changes

### pack.sh

Packs a directory back into an Office file with validation.

**Usage**: `bash ooxml/scripts/pack.sh <input_directory> <office_file> [--force]`

**What it does**: Removes whitespace, creates ZIP, validates with LibreOffice (optional)

## PPTX-Specific Workflows

The only bundled PPTX helper is `pptx/scripts/html2pptx.js`. Other legacy
Python PPTX helpers are not shipped; use manual OOXML inspection/editing,
LibreOffice/PowerPoint export, or the HTML-to-PPTX workflow instead.

### Manual thumbnail export workflow

Generate visual thumbnails with LibreOffice/PowerPoint export when available,
or inspect slide XML/media directly when export tools are unavailable.

### Manual slide reordering workflow

Rearrange slides by editing `ppt/presentation.xml` and related relationship
files after unpacking with `unpack.sh`, then repack with `pack.sh`.

### Manual text inventory workflow

Extract text by inspecting `ppt/slides/slide*.xml`, notes XML, and layout XML.

### Manual text replacement workflow

Replace text in slide XML while preserving surrounding formatting runs.

### html2pptx.js

Convert HTML slides to PowerPoint presentations.

**Usage**: Node.js library for html2pptx workflow

**Features**: HTML to PPTX conversion, chart/table support, custom designs

## Key XML File Structures

### DOCX Files

- `word/document.xml` - Main document content
- `word/comments.xml` - Document comments
- `word/styles.xml` - Document styles
- `word/media/` - Embedded images

### PPTX Files

- `ppt/presentation.xml` - Presentation structure
- `ppt/slides/slide{N}.xml` - Individual slides
- `ppt/notesSlides/notesSlide{N}.xml` - Speaker notes
- `ppt/comments/modernComment_*.xml` - Comments
- `ppt/slideLayouts/` - Layout templates
- `ppt/slideMasters/` - Master templates
- `ppt/theme/` - Theme and styling
- `ppt/media/` - Images and media

## Dependencies

**Python Packages**:

- defusedxml (required) - Safe XML parsing

**System Packages**:

- LibreOffice (optional) - Document validation via `soffice`

**For PPTX Scripts**:

- markitdown, python-pptx (Python)
- pptxgenjs, playwright, sharp (Node.js)
- poppler-utils (system) - PDF conversion for thumbnails

## Best Practices

1. **Always backup originals** before modification
2. **Validate after packing** using LibreOffice or manual testing
3. **Test on samples first** before batch processing
4. **Use version control** for unpacked XML (enables diffs)
5. **Minimal modifications** - only change what's necessary
6. **Preserve structure** - maintain XML structure and attributes

## References

- [Office Open XML Specification](http://www.ecma-international.org/publications/standards/Ecma-376.htm)
- [defusedxml Documentation](https://github.com/tiran/defusedxml)
- [DOCX Skill README](../../docx/README.md)
- [PPTX Skill README](../../pptx/README.md)

---

**Last Updated**: 2025-11-08
**Maintained By**: amplihack project
**Users**: DOCX skill, PPTX skill
