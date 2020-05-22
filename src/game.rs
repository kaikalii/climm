use std::{
    collections::HashSet,
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

use indexmap::IndexMap;
use pathdiff::diff_paths;
use serde_derive::{Deserialize, Serialize};
use walkdir::{DirEntry, WalkDir};

use crate::{
    fomod,
    library::{self},
    utils,
};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GlobalConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_game: Option<String>,
    #[serde(default)]
    pub games: HashSet<String>,
}

impl GlobalConfig {
    pub fn open() -> crate::Result<Self> {
        match fs::read(library::global_config()?) {
            Ok(bytes) => toml::from_slice(&bytes).map_err(Into::into),
            Err(_) => Ok(Self::default()),
        }
    }
    pub fn save(&self) -> crate::Result<()> {
        let string = toml::to_string_pretty(self)?;
        fs::write(library::global_config()?, &string).map_err(Into::into)
    }
    pub fn init_game(
        &mut self,
        name: String,
        folder: PathBuf,
        data: Option<PathBuf>,
        plugins: Option<PathBuf>,
    ) -> crate::Result<()> {
        if self.games.contains(&name) {
            return Err(crate::Error::AlreadyManaged(name));
        }
        self.active_game = Some(name.clone());
        self.games.insert(name.clone());
        Game {
            name: name.clone(),
            config: Config {
                data_folder: data,
                game_folder: folder,
                plugins_file: plugins,
                deployment: DeploymentMethod::default(),
                mods: IndexMap::new(),
            },
        }
        .save()?;
        library::downloads_dir(&name)?;
        println!("Climm initialized {}", name);
        Ok(())
    }
    pub fn game(&self, name: &str) -> crate::Result<Game> {
        if !self.games.contains(name) {
            return Err(crate::Error::UnknownGame(name.into()));
        }
        Game::open(name)
    }
    pub fn active_game(&self) -> crate::Result<Game> {
        self.game(
            self.active_game
                .as_deref()
                .ok_or(crate::Error::NoActiveGame)?,
        )
    }
}

impl Drop for GlobalConfig {
    fn drop(&mut self) {
        if let Err(e) = self.save() {
            println!("Error saving global config: {}", e);
        }
    }
}

fn _true() -> bool {
    true
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ManagedMod {
    pub enabled: bool,
    pub extracted: Option<PathBuf>,
    pub archive: PathBuf,
    pub parts: Vec<PathBuf>,
}

impl ManagedMod {
    pub fn new(archive: PathBuf) -> Self {
        ManagedMod {
            archive,
            ..Self::default()
        }
    }
    pub fn part_paths(&self) -> Vec<PathBuf> {
        if self.parts.is_empty() {
            if let Some(extr) = &self.extracted {
                vec![extr.clone()]
            } else {
                Vec::new()
            }
        } else {
            self.parts.clone()
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum DeploymentMethod {
    Hardlink,
    Symlink,
}

impl Default for DeploymentMethod {
    fn default() -> Self {
        DeploymentMethod::Hardlink
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub game_folder: PathBuf,
    pub data_folder: Option<PathBuf>,
    pub plugins_file: Option<PathBuf>,
    pub deployment: DeploymentMethod,
    pub mods: IndexMap<String, ManagedMod>,
}

pub struct Game {
    pub name: String,
    pub config: Config,
}

const GAME_CONFIG_FILE: &str = "climm.toml";

fn game_config_file(name: &str) -> crate::Result<PathBuf> {
    library::game_dir(name).map(|game_dir| game_dir.join(GAME_CONFIG_FILE))
}

impl Game {
    pub fn config_file(&self) -> crate::Result<PathBuf> {
        game_config_file(&self.name)
    }
    pub fn install_dir(&self) -> PathBuf {
        if let Some(data) = &self.config.data_folder {
            self.config.game_folder.join(data)
        } else {
            self.config.game_folder.clone()
        }
    }
    pub fn open(name: &str) -> crate::Result<Self> {
        let bytes = fs::read(game_config_file(name)?)?;
        let config: Config = toml::from_slice(&bytes)?;
        Ok(Game {
            name: name.into(),
            config,
        })
    }
    pub fn save(&self) -> crate::Result<()> {
        let string = toml::to_string_pretty(&self.config)?;
        fs::write(self.config_file()?, &string).map_err(Into::into)
    }
    pub fn get_mod(&mut self, name: &str) -> crate::Result<(&str, &mut ManagedMod)> {
        let name = name.to_lowercase();
        self.config
            .mods
            .iter_mut()
            .find(|(mod_name, _)| mod_name.to_lowercase().contains(&name))
            .map(|(mod_name, mm)| (mod_name.as_str(), mm))
            .ok_or(crate::Error::UnknownMod(name))
    }
    pub fn add(&mut self, paths: &[PathBuf], mv: bool) -> crate::Result<()> {
        for path in paths {
            if let Some(file_name) = path.file_name() {
                let download_copy = library::downloads_dir(&self.name)?.join(file_name);
                if mv {
                    fs::rename(path, &download_copy)?;
                } else {
                    fs::copy(path, &download_copy)?;
                }
                let mod_name = path.file_stem().unwrap().to_string_lossy().into_owned();
                self.config
                    .mods
                    .insert(mod_name.clone(), ManagedMod::new(download_copy));
                println!("Added {:?}", mod_name);
            }
        }
        Ok(())
    }
    fn extract(&mut self) -> crate::Result<()> {
        for (mod_name, mm) in &mut self.config.mods {
            if mm.enabled && mm.extracted.is_none() {
                let extracted_dir = library::extracted_dir(&self.name, mod_name)?;
                utils::print_erasable(&format!("Extracting {:?}...", mod_name));
                if Command::new("7z")
                    .arg("x")
                    .arg(&mm.archive)
                    .arg(format!("-o{}", extracted_dir.to_string_lossy()))
                    .output()?
                    .status
                    .success()
                {
                    mm.extracted = Some(extracted_dir);
                    println!("Extracted {:?} ", mod_name);
                }
            }
        }
        Ok(())
    }
    fn uninstall(&mut self) -> crate::Result<()> {
        let install_dir = self.install_dir();
        for (_, mm) in &mut self.config.mods {
            if let Some(extracted_dir) = &mm.extracted {
                let extraced_diff = differ(&extracted_dir);
                for entry in WalkDir::new(&extracted_dir) {
                    let file_entry = entry?;
                    utils::remove_path(&install_dir, extraced_diff(&file_entry.path()).unwrap())?;
                }
            }
        }
        Ok(())
    }
    fn install(&mut self) -> crate::Result<()> {
        let install_dir = self.install_dir();
        for (mod_name, mm) in &mut self.config.mods {
            if let (Some(extracted_dir), true) = (&mm.extracted, mm.enabled) {
                let config = WalkDir::new(&extracted_dir)
                    .into_iter()
                    .filter_map(Result::ok)
                    .find(|entry| {
                        entry
                            .path()
                            .file_name()
                            .map_or(false, |name| name == "ModuleConfig.xml")
                    })
                    .map(DirEntry::into_path);
                let install_folders = if !mm.parts.is_empty() {
                    mm.parts.clone()
                } else if config.is_some() {
                    println!(
                        "{:?} has a Fomod installer, but climm does not currently support it. \
                        You can still select which sections you want to install.",
                        mod_name
                    );
                    let paths = fomod::pseudo_fomod(&extracted_dir)?;
                    mm.parts = paths.clone();
                    paths
                } else {
                    vec![extracted_dir.clone()]
                };
                // For each folder
                for folder in install_folders {
                    let folder_diff = differ(&folder);
                    // For each file
                    for entry in WalkDir::new(&folder) {
                        let file_entry = entry?;
                        if file_entry.file_type().is_file() {
                            let extracted_path =
                                folder.join(folder_diff(&file_entry.path()).unwrap());
                            let install_path =
                                install_dir.join(folder_diff(&file_entry.path()).unwrap());
                            utils::create_dirs(&install_path)?;
                            // Deploy
                            match self.config.deployment {
                                DeploymentMethod::Hardlink => {
                                    fs::hard_link(extracted_path, install_path)?
                                }
                                DeploymentMethod::Symlink => {
                                    #[cfg(unix)]
                                    std::os::unix::fs::symlink(extracted_path, install_path)?;
                                    #[cfg(windows)]
                                    std::os::windows::fs::hardlink(extracted_path, install_path)?;
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
    pub fn write_plugins(&mut self) -> crate::Result<()> {
        if let Some(plugins) = &self.config.plugins_file {
            let mut file = File::create(plugins)?;
            for (_, mm) in &self.config.mods {
                if mm.enabled {
                    for path in mm.part_paths() {
                        for entry in WalkDir::new(path).into_iter().filter_map(Result::ok) {
                            if let Some(ext) = entry.path().extension() {
                                if ["esp", "esm", "esl"].contains(&ext.to_string_lossy().as_ref()) {
                                    writeln!(
                                        file,
                                        "*{}",
                                        entry.path().file_name().unwrap().to_string_lossy()
                                    )?;
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
    pub fn deploy(&mut self) -> crate::Result<()> {
        self.extract()?;
        utils::print_erasable("Deploying...");
        self.uninstall()?;
        self.install()?;
        self.write_plugins()?;
        println!("Deployed");
        Ok(())
    }
}

impl Drop for Game {
    fn drop(&mut self) {
        if let Err(e) = self.save() {
            println!("Error saving config: {}", e);
        }
    }
}

fn differ<P>(top: &P) -> impl Fn(&'_ Path) -> Option<PathBuf> + '_
where
    P: AsRef<Path>,
{
    move |path| diff_paths(path, top)
}
