//! A mod's MSBuild project, evaluated by MSBuild rather than guessed from XML. This writes the question
//! ([`evaluation_arguments`]) and reads the answer ([`read_evaluation`]); running the program is the caller's. Also the
//! files that can change an evaluation or a build ([`Stamp`], [`changed_since`]), and the .NET SDK ([`dotnet`]).

use file_ops::stamp::FileStamp;
use serde_json::Value;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub mod dotnet;

/// What MSBuild says a project builds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvaluatedProject {
    pub target_path: PathBuf,
    pub name: String,
    pub version: Option<String>,
    /// A library that references MelonLoader.
    pub is_mod: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Evaluation {
    /// None when MSBuild answered, but with no absolute path to a DLL.
    pub project: Option<EvaluatedProject>,
    /// The files MSBuild read on the way, for a [`Stamp`].
    pub imports: Vec<PathBuf>,
}

/// What to give `dotnet` for it to print the evaluation [`read_evaluation`] reads.
pub fn evaluation_arguments(file: &Path) -> Vec<OsString> {
    let mut arguments = vec![OsString::from("msbuild"), file.as_os_str().to_owned()];
    arguments.extend(
        [
            "-nologo",
            "-getProperty:TargetPath",
            "-getProperty:OutputType",
            "-getProperty:AssemblyName",
            "-getProperty:Version",
            "-getProperty:MSBuildAllProjects",
            "-getItem:Reference",
            "-getItem:PackageReference",
            "-p:Configuration=Release",
        ]
        .map(OsString::from),
    );
    arguments
}

/// None when the output isn't JSON at all: a first-run banner, which says nothing about the project.
pub fn read_evaluation(file: &Path, output: &str) -> Option<Evaluation> {
    let root: Value = serde_json::from_str(output).ok()?;
    let Some(properties) = root.get("Properties").filter(|value| value.is_object()) else {
        return Some(Evaluation {
            project: None,
            imports: Vec::new(),
        });
    };
    let imports = text(properties, "MSBuildAllProjects")
        .into_iter()
        .flat_map(|value| value.split(';'))
        .map(str::trim)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .collect();
    let target = PathBuf::from(text(properties, "TargetPath").unwrap_or_default());
    if !target.is_absolute() || !target.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("dll")) {
        return Some(Evaluation { project: None, imports });
    }
    let library = text(properties, "OutputType").is_some_and(|kind| kind.eq_ignore_ascii_case("Library"));
    let items = |kind| root.get("Items").and_then(|items| items.get(kind)).and_then(Value::as_array).into_iter().flatten();
    let melon = items("Reference").any(|item| text(item, "Filename").is_some_and(|name| name.eq_ignore_ascii_case("MelonLoader")))
        || items("PackageReference").any(|item| text(item, "Identity").is_some_and(|name| name.to_ascii_lowercase().starts_with("lavagang.melonloader")));
    Some(Evaluation {
        project: Some(EvaluatedProject {
            target_path: target,
            name: text(properties, "AssemblyName")
                .map(str::to_owned)
                .unwrap_or_else(|| file.file_stem().unwrap_or_default().to_string_lossy().into_owned()),
            version: text(properties, "Version").map(str::to_owned),
            is_mod: library && melon,
        }),
        imports,
    })
}

fn text<'a>(owner: &'a Value, name: &str) -> Option<&'a str> {
    owner.get(name)?.as_str().map(str::trim).filter(|text| !text.is_empty())
}

/// One spelling for one path, whatever its case or form.
pub fn path_key(path: &Path) -> String {
    file_ops::paths::normalized(path).to_string_lossy().to_lowercase()
}

/// The files an evaluation depends on, as they are now. A project below `folder` evaluates afresh when this differs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stamp(Vec<(String, Option<FileStamp>)>);

impl Stamp {
    pub fn of(file: &Path, folder: &Path, imports: &[PathBuf]) -> io::Result<Self> {
        let mut paths = vec![file.to_path_buf()];
        paths.extend(build_props(file, Some(folder)));
        paths.extend(above(file, "Directory.Packages.props", Some(folder)));
        paths.extend(above(file, "global.json", None));
        paths.extend_from_slice(imports);
        let mut stamp = BTreeMap::new();
        for path in paths {
            stamp.insert(path_key(&path), file_ops::read::if_present(FileStamp::of(&path))?);
        }
        Ok(Self(stamp.into_iter().collect()))
    }
}

/// Whether a source file beside the project, or a build file above it up to `sources`, was written after `built`.
pub fn changed_since(project_file: &Path, sources: Option<&Path>, built: SystemTime) -> io::Result<bool> {
    let mut source_files = files(project_file.parent().unwrap_or(project_file), None)?;
    source_files.extend(build_props(project_file, sources));
    Ok(source_files.iter().any(|file| file_ops::stamp::written(file).ok().is_some_and(|time| time > built)))
}

fn above(project: &Path, name: &str, top: Option<&Path>) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let top = top.map(path_key);
    for folder in project.parent().into_iter().flat_map(Path::ancestors) {
        let path = folder.join(name);
        if path.is_file() {
            found.push(path);
        }
        if top.as_ref().is_some_and(|top| *top == path_key(folder)) {
            break;
        }
    }
    found
}

fn build_props(project: &Path, sources: Option<&Path>) -> Vec<PathBuf> {
    let mut props = above(project, "Directory.Build.props", sources);
    props.extend(above(project, "Directory.Build.targets", sources));
    props
}

/// The `.csproj` files under `folder`, at most `depth` folders down, in name order.
pub fn project_files(folder: &Path, depth: usize) -> io::Result<Vec<PathBuf>> {
    files(folder, Some(depth))
}

/// With a depth, collect projects; without one, collect all source files. Reparse folders are excluded so a linked
/// output or directory cycle cannot escape the user's sources tree.
fn files(folder: &Path, depth: Option<usize>) -> io::Result<Vec<PathBuf>> {
    let mut entries = fs::read_dir(folder)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name().to_ascii_lowercase());
    let mut found = Vec::new();
    for entry in entries {
        let path = entry.path();
        let metadata = entry.metadata()?;
        if metadata.is_file() && (depth.is_none() || path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("csproj"))) {
            found.push(path);
        } else if metadata.is_dir() && depth != Some(0) && searched(&entry) {
            found.extend(files(&path, depth.map(|depth| depth - 1))?);
        }
    }
    Ok(found)
}

fn searched(entry: &fs::DirEntry) -> bool {
    let name = entry.file_name().to_string_lossy().to_lowercase();
    !(name.starts_with('.') || name == "bin" || name == "obj") && entry.metadata().is_ok_and(|metadata| file_ops::paths::is_plain_directory(&metadata))
}

/// The first five different errors in a build's output, one a line, with `folder` taken off their paths.
pub fn compiler_errors(output: &str, folder: &Path) -> String {
    let prefix = format!("{}{}", folder.display(), std::path::MAIN_SEPARATOR);
    let mut errors = Vec::new();
    for line in output.lines().map(str::trim).filter(|line| line.contains(": error ")) {
        let line = if line.ends_with(']') {
            line.rsplit_once(" [").map_or(line, |(error, _)| error)
        } else {
            line
        };
        let line = if line.get(..prefix.len()).is_some_and(|start| start.eq_ignore_ascii_case(&prefix)) {
            &line[prefix.len()..]
        } else {
            line
        };
        if !errors.contains(&line) {
            errors.push(line);
            if errors.len() == 5 {
                break;
            }
        }
    }
    errors.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    fn touch(root: &Path, relative: &str) -> PathBuf {
        let file = root.join(relative);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "<Project />").unwrap();
        file
    }

    fn at(seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
    }

    fn modified_at(file: &Path, seconds: u64) {
        fs::File::options()
            .write(true)
            .open(file)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(at(seconds)))
            .unwrap();
    }

    #[test]
    fn evaluation_identifies_melon_packages_but_never_guesses_relative_output_paths() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("Loom.csproj");
        let mut answer = json!({
            "Properties": { "TargetPath": temp.path().join("bin/Loom.dll"), "OutputType": "Library", "AssemblyName": "CustomName", "Version": " 0.7.0 " },
            "Items": { "PackageReference": [{ "Identity": "LavaGang.MelonLoader.net6" }] },
        });
        let found = read_evaluation(&file, &answer.to_string()).unwrap().project.unwrap();
        assert!(found.is_mod);
        assert_eq!(found.name, "CustomName");
        assert_eq!(found.version.as_deref(), Some("0.7.0"));
        answer["Properties"]["TargetPath"] = "bin/Loom.dll".into();
        assert!(read_evaluation(&file, &answer.to_string()).unwrap().project.is_none());
        assert!(read_evaluation(&file, "Welcome to .NET").is_none());
    }

    #[test]
    fn imports_survive_an_unusable_target_and_an_answer_without_properties_has_none() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("Loom.csproj");
        let imported = temp.path().join("custom.targets");
        let answer = json!({ "Properties": { "TargetPath": "bin/Loom.dll", "MSBuildAllProjects": format!("relative.props;{}", imported.display()) } });
        let evaluation = read_evaluation(&file, &answer.to_string()).unwrap();
        assert_eq!(
            evaluation,
            Evaluation {
                project: None,
                imports: vec![imported]
            }
        );
        assert_eq!(
            read_evaluation(&file, "{}").unwrap(),
            Evaluation {
                project: None,
                imports: Vec::new()
            }
        );
    }

    #[test]
    fn the_questions_first_arguments_are_the_command_and_the_project() {
        let arguments = evaluation_arguments(Path::new("mods/Loom.csproj"));
        assert_eq!(arguments[0], "msbuild");
        assert_eq!(arguments[1], "mods/Loom.csproj");
        assert!(arguments.contains(&"-p:Configuration=Release".into()));
    }

    #[test]
    fn projects_are_limited_by_depth_and_skip_outputs_and_hidden_folders() {
        let temp = tempfile::tempdir().unwrap();
        let wanted = touch(temp.path(), "a/b/c/DepthThree.csproj");
        for relative in [
            "a/b/c/d/TooDeep.csproj",
            "loom/bin/Copied.csproj",
            "loom/OBJ/Copied.csproj",
            ".git/Hidden.csproj",
            "a/Readme.md",
        ] {
            touch(temp.path(), relative);
        }
        assert_eq!(project_files(temp.path(), 3).unwrap(), [wanted]);
    }

    #[test]
    fn a_stamp_changes_with_the_project_its_imports_and_the_build_files_above_it() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("mods");
        let file = touch(&root, "loom/src/Loom.csproj");
        let imported = touch(temp.path(), "sdk/custom.targets");
        modified_at(&imported, 1);
        let imports = [imported.clone()];
        let mut seen = Stamp::of(&file, &root, &imports).unwrap();
        assert_eq!(seen, Stamp::of(&file, &root, &imports).unwrap());
        let changes: [&dyn Fn(); 5] = [
            &|| modified_at(&imported, 2),
            &|| drop(touch(&root, "Directory.Build.props")),
            &|| drop(touch(&root, "Directory.Packages.props")),
            &|| drop(touch(temp.path(), "global.json")),
            &|| modified_at(&file, 3),
        ];
        for change in changes {
            change();
            let now = Stamp::of(&file, &root, &imports).unwrap();
            assert_ne!(seen, now);
            seen = now;
        }
    }

    #[test]
    fn build_files_above_the_sources_folder_do_not_count_except_global_json() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("mods");
        let file = touch(&root, "Loom.csproj");
        let before = Stamp::of(&file, &root, &[]).unwrap();
        touch(temp.path(), "Directory.Build.props");
        assert_eq!(before, Stamp::of(&file, &root, &[]).unwrap());
        touch(temp.path(), "global.json");
        assert_ne!(before, Stamp::of(&file, &root, &[]).unwrap());
    }

    #[test]
    fn a_change_since_a_build_excludes_generated_output() {
        let temp = tempfile::tempdir().unwrap();
        let file = touch(temp.path(), "loom/src/Loom.csproj");
        let source = touch(temp.path(), "loom/src/Entry.cs");
        let props = touch(temp.path(), "loom/Directory.Build.props");
        for file in [&file, &source, &props] {
            modified_at(file, 1);
        }
        modified_at(&touch(temp.path(), "loom/src/obj/generated.cs"), 100);
        modified_at(&touch(temp.path(), "loom/src/.git/index"), 100);
        assert!(!changed_since(&file, Some(temp.path()), at(2)).unwrap());
        modified_at(&source, 3);
        assert!(changed_since(&file, Some(temp.path()), at(2)).unwrap());
        modified_at(&source, 1);
        modified_at(&props, 3);
        assert!(changed_since(&file, Some(temp.path()), at(2)).unwrap());
    }

    #[test]
    fn compiler_errors_are_distinct_limited_and_short() {
        let output = (0..9).map(|index| format!("file.cs: error E{index}: broken\n")).collect::<String>();
        assert_eq!(compiler_errors(&output, Path::new("unused")).lines().count(), 5);
        let folder = Path::new("mods");
        let line = format!(
            "{}: error CS1002: ; expected [{}]\n",
            folder.join("Entry.cs(12,5)").display(),
            folder.join("Loom.csproj").display()
        );
        assert_eq!(compiler_errors(&line.repeat(2), folder), "Entry.cs(12,5): error CS1002: ; expected");
    }
}
