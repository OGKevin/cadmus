use cadmus_core::anyhow::{Context, Error, format_err};
use cadmus_core::chrono::NaiveDateTime;
use cadmus_core::db::Database;
use cadmus_core::device::CURRENT_DEVICE;
use cadmus_core::helpers::datetime_format;
// use cadmus_core::library::importer;
use cadmus_core::library::Library;
use cadmus_core::metadata::{consolidate, rename_from_info};
use cadmus_core::metadata::{extract_metadata_from_document, extract_metadata_from_filename};
use cadmus_core::settings::ImportSettings;
// use cadmus_core::view::{Event, ViewId, ID_FEEDER};
use getopts::Options;
use std::env;
use std::path::Path;
// use std::sync::mpsc;

fn main() -> Result<(), Error> {
    let args: Vec<String> = env::args().skip(1).collect();

    let mut opts = Options::new();

    opts.optflag("h", "help", "Print this help message.");
    opts.optflag("I", "import", "Import new files or update existing files.");
    opts.optflag(
        "C",
        "clean-up",
        "Remove reading states with unknown fingerprints.",
    );
    opts.optflag(
        "E",
        "extract-metadata-document",
        "Extract metadata from documents.",
    );
    opts.optflag(
        "F",
        "extract-metadata-filename",
        "Extract metadata from filenames.",
    );
    opts.optflag(
        "S",
        "consolidate",
        "Autocorrect simple typographic mistakes.",
    );
    opts.optflag(
        "N",
        "rename-from-info",
        "Rename files based on their information.",
    );
    opts.optopt(
        "k",
        "allowed-kinds",
        "Comma separated list of allowed kinds.",
        "ALLOWED_KINDS",
    );
    opts.optopt(
        "e",
        "metadata-kinds",
        "Comma separated list of metadata kinds.",
        "METADATA_KINDS",
    );
    opts.optopt(
        "a",
        "added-after",
        "Only process entries added after the given date-time.",
        "ADDED_DATETIME",
    );
    opts.optopt(
        "m",
        "library-mode",
        "The library mode (`database` or `filesystem`).",
        "LIBRARY_MODE",
    );

    let matches = opts
        .parse(&args)
        .context("failed to parse the command line arguments")?;

    if matches.opt_present("h") {
        println!("{}", opts.usage("Usage: cadmus-import -h|-I|-C|-EFSN [-k ALLOWED_KINDS] [-e METADATA_KINDS] [-a ADDED_DATETIME] [-m LIBRARY_MODE] LIBRARY_PATH"));
        return Ok(());
    }

    if matches.free.is_empty() {
        return Err(format_err!("missing required argument: library path"));
    }

    let library_path = Path::new(&matches.free[0]);

    let mut import_settings = ImportSettings {
        metadata_kinds: ["epub"].iter().map(|k| k.to_string()).collect(),
        ..Default::default()
    };

    if let Some(allowed_kinds) = matches.opt_str("k").map(|v| {
        v.split(',')
            .filter_map(
                |k| match k.parse::<cadmus_core::settings::FileExtension>() {
                    Ok(ext) => Some(ext),
                    Err(e) => {
                        eprintln!("Warning: {e}, skipping");
                        None
                    }
                },
            )
            .collect()
    }) {
        import_settings.allowed_kinds = allowed_kinds;
    }

    if let Some(metadata_kinds) = matches
        .opt_str("e")
        .map(|v| v.split(',').map(|k| k.to_string()).collect())
    {
        import_settings.metadata_kinds = metadata_kinds;
    }

    let added_after = matches
        .opt_str("a")
        .as_ref()
        .and_then(|v| NaiveDateTime::parse_from_str(v, datetime_format::FORMAT).ok());

    let database = Database::new(CURRENT_DEVICE.resolve_db_path())?;
    let library_name = library_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("Imported Library")
        .to_string();
    let mut library = Library::new(library_path, &database, &library_name)?;

    if matches.opt_present("I") {
        // let notif_id = ViewId::MessageNotif(ID_FEEDER.next());
        // let (hub, _rx) = mpsc::channel::<Event>();
        // this should be refactored to either be made into a plugin and talk via the API
        // or incorporated into core.
        // for now, just comment out.
        // importer::run(
        //     &library.db,
        //     library.library_id,
        //     &library.home,
        //     &import_settings,
        //     &hub,
        //     notif_id,
        // );
    } else if matches.opt_present("C") {
        library.clean_up();
    } else {
        let opt_extract_metadata_document = matches.opt_present("E");
        let opt_extract_metadata_filename = matches.opt_present("F");
        let opt_consolidate = matches.opt_present("S");
        let opt_rename_from_info = matches.opt_present("N");

        library.apply(|path, info| {
            if added_after.is_none_or(|added| info.added >= added) {
                if opt_extract_metadata_document
                    && import_settings.metadata_kinds.contains(&info.file.kind)
                {
                    extract_metadata_from_document(path, info);
                }

                if opt_extract_metadata_filename {
                    extract_metadata_from_filename(path, info);
                }

                if opt_consolidate {
                    consolidate(path, info);
                }

                if opt_rename_from_info {
                    rename_from_info(path, info);
                }
            }
        });
    }

    Ok(())
}
