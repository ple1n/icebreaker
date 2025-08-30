use crate::directory;
use crate::model;
use crate::request;
use crate::Error;
use crate::Settings;

use decoder::{decode, encode, Value};
use langchain_rust::llm::nanogpt::NanoGPT;
use langchain_rust::llm::OpenAIConfig;
use langchain_rust::llm::OpenAIConfigSerde;
use log::info;
use rcu_cell::ArcRCU;
use rcu_cell::ArcRCUNonNull;
use serde::{Deserialize, Serialize};
use sipper::{sipper, Sipper, Straw};
use tokio::fs;
use tokio::sync::watch;

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const HF_URL: &str = "https://huggingface.co";
const API_URL: &str = "https://huggingface.co/api";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct APIAccess {
    pub openai_compat: Option<OpenAIConfigSerde>,
    pub kind: APIType,
}

#[derive(Debug, Clone)]
pub struct HFModel {
    pub id: Id,
    pub last_modified: chrono::DateTime<chrono::Local>,
    pub downloads: Downloads,
    pub likes: Likes,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelOnline {
    pub endpoint_id: EndpointId,
    pub cost: Option<Cost>,
    /// All the information needed to access this API
    pub config: APIAccess,
    pub state_check: ArcRCUNonNull<StatusCheck>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum StatusCheck {
    #[default]
    Unchecked,
    CheckingStatus,
    Up,
    Down,
}

#[derive(Debug, Clone)]
pub enum Model {
    HF(HFModel),
    API(ModelOnline),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
pub enum APIType {
    /// Dispatches to nanogpt impl in async_openai
    NanoGPT,
    OpenAI,
    #[default]
    OpenAICompatible,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cost {
    pub prompt: Quantity,
    pub completion: Quantity,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quantity {
    pub num: f64,
    pub unit: Currency,
    pub denom: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Currency {
    USD,
}

impl Quantity {
    pub fn usd_per_1m(n: f64) -> Self {
        Self {
            num: n,
            unit: Currency::USD,
            denom: 1e6,
        }
    }
}

pub type ModelsMap = HashMap<model::EndpointId, Model>;

impl Model {
    pub async fn list(api: Arc<Library>) -> Result<ModelsMap, Error> {
        let mut resp = ModelsMap::new();

        for (id, api) in api.api_src.iter() {
            match &api.kind {
                APIType::NanoGPT => {
                    let nanogpt: NanoGPT<OpenAIConfig> =
                        NanoGPT::new(api.openai_compat.clone().unwrap().into());
                    let models = nanogpt.get_models(true).await?;
                    for m in models.data {
                        let _ = resp.insert(
                            EndpointId::Remote {
                                api_type: APIType::NanoGPT,
                                id: Id(m.id.clone()),
                            },
                            Model::API(ModelOnline {
                                endpoint_id: EndpointId::Remote {
                                    api_type: APIType::NanoGPT,
                                    id: Id(m.id),
                                },
                                cost: m.pricing.as_ref().map(|p| Cost {
                                    prompt: Quantity::usd_per_1m(p.prompt),
                                    completion: Quantity::usd_per_1m(p.completion),
                                }),
                                config: api.clone(),
                                state_check: Default::default(),
                            }),
                        );
                    }
                }
                _ => todo!(),
            }
        }

        Ok(resp)
    }
    /// Return ID of the form repo/name
    pub fn slash_id(&self) -> &Id {
        match &self {
            Self::API(m) => m.endpoint_id.slash_id(),
            Self::HF(m) => &m.id,
        }
    }
    pub async fn search(_query: String) -> Result<Vec<Self>, Error> {
        Ok(vec![])
    }

    pub fn endpoint_id(&self) -> EndpointId {
        match self {
            Self::HF(m) => EndpointId::Local(m.id.clone()),
            Self::API(m) => m.endpoint_id.clone(),
        }
    }
}

impl HFModel {
    pub fn endpoint_id(&self) -> EndpointId {
        EndpointId::Local(self.id.clone())
    }

    pub async fn list() -> Result<Vec<Self>, Error> {
        Self::search(String::new()).await
    }

    pub async fn search(query: String) -> Result<Vec<Self>, Error> {
        let client = reqwest::Client::new();

        let request = client.get(format!("{API_URL}/models")).query(&[
            ("search", query.as_ref()),
            ("filter", "text-generation"),
            ("filter", "gguf"),
            ("limit", "100"),
            ("full", "true"),
        ]);

        #[derive(Deserialize)]
        struct Response {
            id: Id,
            #[serde(rename = "lastModified")]
            last_modified: chrono::DateTime<chrono::Local>,
            downloads: Downloads,
            likes: Likes,
            gated: Gated,
        }

        #[derive(Deserialize, PartialEq, Eq)]
        #[serde(untagged)]
        enum Gated {
            Bool(bool),
            Other(String),
        }

        let response = request.send().await?;
        let mut models: Vec<Response> = response.json().await?;

        models.retain(|model| model.gated == Gated::Bool(false));

        Ok(models
            .into_iter()
            .map(|model| Self {
                id: model.id.clone(),
                last_modified: model.last_modified,
                downloads: model.downloads,
                likes: model.likes,
            })
            .collect())
    }
}

impl fmt::Display for HFModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.id.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct Id(pub String);

impl Id {
    pub fn name(&self) -> &str {
        self.0
            .split_once('/')
            .map(|(_author, name)| name)
            .unwrap_or(&self.0)
    }

    pub fn author(&self) -> &str {
        self.0
            .split_once('/')
            .map(|(author, _name)| author)
            .unwrap_or(&self.0)
    }
}

#[derive(Debug, Clone)]
pub struct Details {
    pub last_modified: chrono::DateTime<chrono::Local>,
    pub downloads: Downloads,
    pub likes: Likes,
    pub architecture: Option<String>,
    pub parameters: Parameters,
}

impl Details {
    pub async fn fetch(id: EndpointId) -> Result<Self, Error> {
        let id = match id {
            EndpointId::Local(d) => d,
            _ => unreachable!(),
        };

        #[derive(Deserialize)]
        struct Response {
            #[serde(rename = "lastModified")]
            last_modified: chrono::DateTime<chrono::Local>,
            downloads: Downloads,
            likes: Likes,
            gguf: Gguf,
        }

        #[derive(Deserialize)]
        struct Gguf {
            #[serde(default)]
            architecture: Option<String>,
            total: u64,
        }

        let client = reqwest::Client::new();
        let request = client.get(format!("{}/models/{}", API_URL, id.0));

        let response: Response = request.send().await?.error_for_status()?.json().await?;

        Ok(Self {
            last_modified: response.last_modified,
            downloads: response.downloads,
            likes: response.likes,
            architecture: response.gguf.architecture,
            parameters: Parameters(response.gguf.total),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct Downloads(u64);

impl fmt::Display for Downloads {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            1_000_000.. => {
                write!(f, "{:.2}M", (self.0 as f32 / 1_000_000_f32))
            }
            1_000.. => {
                write!(f, "{:.2}k", (self.0 as f32 / 1_000_f32))
            }
            _ => {
                write!(f, "{}", self.0)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct Likes(u64);

impl fmt::Display for Likes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
pub struct Parameters(u64);

impl fmt::Display for Parameters {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0.ilog10() {
            0..3 => write!(f, "{}", self.0),
            3..6 => write!(f, "{}K", self.0 / 1000),
            6..9 => write!(f, "{}M", self.0 / 1_000_000),
            9..12 => write!(f, "{}B", self.0 / 1_000_000_000),
            _ => write!(f, "{}T", self.0 / 1_000_000_000),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct File {
    pub model: Id,
    pub name: String,
    #[serde(default)]
    pub size: Option<Size>,
}

impl File {
    pub fn endpoint(&self) -> EndpointId {
        EndpointId::Local(self.model.clone())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FileAndAPI {
    pub file: Option<File>,
    pub api: Option<ModelOnline>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileOrAPI {
    File(File),
    API(ModelOnline),
}

impl FileAndAPI {
    pub fn slash_id(&self) -> &Id {
        if let Some(f) = &self.file {
            &f.model
        } else if let Some(a) = &self.api {
            a.endpoint_id.slash_id()
        } else {
            panic!("FileOrAPI is empty");
        }
    }
}

impl PartialEq for ModelOnline {
    fn eq(&self, other: &Self) -> bool {
        self.endpoint_id == other.endpoint_id
    }
}

impl Eq for ModelOnline {}

impl FileAndAPI {
    pub async fn list(directory: &Directory) -> Result<Vec<Self>, Error> {
        let mut models = Vec::new();
        let dir = directory.as_ref();
        fs::create_dir_all(dir).await?;
        let mut entries = fs::read_dir(dir).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
                let content = fs::read_to_string(&path).await?;
                let model_online: ModelOnline = serde_json::from_str(&content)?;
                models.push(FileAndAPI {
                    file: None,
                    api: Some(model_online),
                });
            }
        }

        Ok(models)
    }

    pub fn download<'a>(
        &'a self,
        directory: &'a Directory,
    ) -> impl Straw<PathBuf, request::Progress, Error> + 'a {
        sipper(async move |sender| match self {
            FileAndAPI { api: Some(ap), .. } => {
                let id = match &ap.endpoint_id {
                    EndpointId::Local(id) => id,
                    EndpointId::Remote { id, .. } => id,
                };
                let json_path = directory.0.join(format!("{}.json", id.0.replace('/', "_")));
                if !json_path.exists() {
                    let json_content = serde_json::to_string_pretty(ap)?;
                    fs::write(&json_path, json_content).await?;
                }
                Ok(json_path)
            }
            FileAndAPI {
                file: Some(file), ..
            } => file.download(directory, sender).await,
            _ => Err(std::io::Error::new(std::io::ErrorKind::Other, "FileOrAPI is empty").into()),
        })
    }
}

impl File {
    pub async fn list(id: Id) -> Result<Files, Error> {
        let client = reqwest::Client::new();
        let request = client.get(format!("{}/models/{}/tree/main", API_URL, id.0));

        #[derive(Debug, Deserialize)]
        struct Entry {
            r#type: String,
            path: String,
            size: u64,
        }

        let entries: Vec<Entry> = request.send().await?.error_for_status()?.json().await?;
        let mut files: BTreeMap<Bits, Vec<File>> = BTreeMap::new();

        for entry in entries {
            if entry.r#type != "file" || !entry.path.ends_with(".gguf") {
                continue;
            }

            let file_stem = entry.path.trim_end_matches(".gguf");
            let variant = file_stem.rsplit(['-', '.']).next().unwrap_or(file_stem);
            let precision = variant
                .split('_')
                .next()
                .unwrap_or(variant)
                .trim_start_matches("IQ")
                .trim_start_matches("Q")
                .trim_start_matches("BF")
                .trim_start_matches("F")
                .parse()
                .map(Bits);

            let Ok(precision) = precision else {
                continue;
            };

            let files = files.entry(precision).or_default();

            files.push(File {
                model: id.clone(),
                name: entry.path,
                size: Some(Size(entry.size)),
            })
        }

        Ok(files)
    }

    pub async fn download<'a>(
        &'a self,
        directory: &'a Directory,
        sender: sipper::Sender<request::Progress>,
    ) -> Result<PathBuf, Error> {
        let old_path = Directory::old().0.join(&self.name);
        let directory = directory.0.join(&self.model.0);
        let model_path = directory.join(&self.name);

        fs::create_dir_all(&directory).await?;

        if fs::try_exists(&model_path).await? {
            let file_metadata = fs::metadata(&model_path).await?;

            if self.size.is_none_or(|size| size == file_metadata.len()) {
                return Ok(model_path);
            }

            fs::remove_file(&model_path).await?;
        }

        if fs::copy(&old_path, &model_path).await.is_ok() {
            let _ = fs::remove_file(old_path).await;
            return Ok(model_path);
        }

        let url = format!(
            "{}/{id}/resolve/main/{filename}?download=true",
            HF_URL,
            id = self.model.0,
            filename = self.name
        );

        let temp_path = model_path.with_extension("tmp");

        request::download_file(url, &temp_path).run(sender).await?;
        fs::rename(temp_path, &model_path).await?;

        Ok(model_path)
    }

    pub fn decode(value: decoder::Value) -> decoder::Result<Self> {
        use decoder::decode::{map, string, u64};

        let mut file = map(value)?;

        Ok(Self {
            model: Id(file.required("model", string)?),
            name: file.required("name", string)?,
            size: file.optional("size", u64)?.map(Size),
        })
    }

    pub fn encode(self) -> decoder::Value {
        use decoder::encode::{map, string};

        map([("model", string(self.model.0)), ("name", string(self.name))]).into()
    }

    pub fn variant(&self) -> Option<&str> {
        self.name
            .trim_end_matches(".gguf")
            .rsplit(['-', '.'])
            .next()
    }

    pub fn relative_path(&self) -> PathBuf {
        PathBuf::from(&self.model.0).join(&self.name)
    }
}

impl fmt::Display for File {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
    }
}

pub type Files = BTreeMap<Bits, Vec<File>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Bits(u64);

impl fmt::Display for Bits {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-bit", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Size(u64);

impl fmt::Display for Size {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0.ilog10() {
            0..3 => write!(f, "{} B", self.0),
            3..6 => write!(f, "{} KB", self.0 / 1000),
            6..9 => write!(f, "{} MB", self.0 / 1_000_000),
            9..12 => write!(f, "{} GB", self.0 / 1_000_000_000),
            _ => write!(f, "{} TB", self.0 / 1_000_000_000_000),
        }
    }
}

impl PartialEq<u64> for Size {
    fn eq(&self, other: &u64) -> bool {
        &self.0 == other
    }
}

#[derive(Debug, Clone)]
pub struct Readme {
    pub markdown: String,
}

impl Readme {
    pub async fn fetch(id: Id) -> Result<Self, Error> {
        let response = reqwest::get(format!(
            "{url}/{id}/raw/main/README.md",
            url = HF_URL,
            id = id.0
        ))
        .await?;

        Ok(Self {
            markdown: response.text().await?,
        })
    }
}

use std::collections::HashMap;
#[derive(Debug, Clone, Default)]
pub struct Library {
    directory: Directory,
    pub api_src: HashMap<APIType, APIAccess>,
    pub files: HashMap<EndpointId, FileOrAPI>,
    pub bookmarks: Vec<EndpointId>,
}

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct APIBookmarks {
    pub api_src: HashMap<APIType, APIAccess>,
    pub apis: HashMap<EndpointId, ModelOnline>,
    pub bookmarks: Vec<EndpointId>,
}

#[derive(Hash, PartialEq, Eq, Debug, Clone, Serialize, Deserialize)]
pub enum EndpointId {
    Local(Id),
    Remote { api_type: APIType, id: Id },
}

impl EndpointId {
    pub fn slash_id(&self) -> &Id {
        match self {
            Self::Local(id) => id,
            Self::Remote { id, .. } => id,
        }
    }
}

impl Library {
    pub async fn scan(settings: Settings) -> Result<Self, Error> {
        let directory = &settings.library;
        let bookmarks_file = settings.bookmarks();

        let mut files: HashMap<EndpointId, FileOrAPI> = HashMap::new();
        let directory = directory.as_ref();
        fs::create_dir_all(directory).await?;

        let mut list = fs::read_dir(directory).await?;

        while let Some(author) = list.next_entry().await? {
            if !author.file_type().await?.is_dir() {
                continue;
            }

            let mut directory = fs::read_dir(author.path()).await?;

            while let Some(model) = directory.next_entry().await? {
                if !model.file_type().await?.is_dir() {
                    continue;
                }

                let mut directory = fs::read_dir(model.path()).await?;

                while let Some(file) = directory.next_entry().await? {
                    if !file.file_type().await?.is_file()
                        || file.path().extension().unwrap_or_default() != "gguf"
                    {
                        continue;
                    }
                    let id = Id(format!(
                        "{}/{}",
                        author.file_name().display(),
                        model.file_name().display(),
                    ));
                    let f_id = EndpointId::Local(id.clone());
                    let file = FileOrAPI::File(File {
                        model: id,
                        name: file.file_name().display().to_string(),
                        size: Some(Size(file.metadata().await?.len())),
                    });

                    let _ = files.insert(f_id, file);
                }
            }
        }

        info!("reading {:?}", &bookmarks_file);
        let bookmarks: APIBookmarks = match fs::read_to_string(&bookmarks_file).await {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => Default::default(),
        };

        Ok(Self {
            directory: Directory(directory.to_path_buf()),
            files: bookmarks
                .apis
                .into_iter()
                .map(|(id, api)| (id, FileOrAPI::API(api)))
                .chain(files)
                .collect(),
            api_src: bookmarks.api_src,
            bookmarks: bookmarks.bookmarks,
        })
    }

    pub async fn save_bookmarks(self: Arc<Self>, settings: Settings) -> Result<Arc<Self>, Error> {
        let bookmarks_file = settings.bookmarks();
        let api_bookmarks = APIBookmarks {
            api_src: self.api_src.clone(),
            apis: self
                .files
                .iter()
                .filter_map(|(id, file_or_api)| match file_or_api {
                    FileOrAPI::API(api) => Some((id.clone(), api.clone())),
                    _ => None,
                })
                .collect(),
            bookmarks: self.bookmarks.clone(),
        };
        let json = serde_json::to_string_pretty(&api_bookmarks)?;
        info!("writing bookmarks to {:?}", &bookmarks_file);
        fs::write(bookmarks_file, json).await?;

        Ok(self)
    }

    pub async fn status_check(self: Arc<Self>, id: EndpointId) -> Result<(), Error> {
        
        
        Ok(())
    }

    pub fn directory(&self) -> &Directory {
        &self.directory
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Directory(PathBuf);

impl Directory {
    pub fn decode(value: Value) -> decoder::Result<Self> {
        decode::string(value).map(PathBuf::from).map(Self)
    }

    pub fn encode(&self) -> Value {
        encode::string(self.0.to_string_lossy())
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    fn old() -> Self {
        Directory(PathBuf::from("./models"))
    }
}

impl Default for Directory {
    fn default() -> Self {
        Self(directory::data().join("models"))
    }
}

impl AsRef<Path> for Directory {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl fmt::Display for Quantity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.2}", self.num)
    }
}

impl FileOrAPI {
    pub fn slash_id(&self) -> &Id {
        match self {
            Self::File(f) => &f.model,
            Self::API(a) => a.endpoint_id.slash_id(),
        }
    }
}
