//! Private immutable artifacts, read and hashed through the same descriptor.
//! Registration is provenance within this user account, not privilege isolation.
use super::schema::*;
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize, de::{self, Visitor, MapAccess, SeqAccess}};
use serde_json::{Value, Map};
use sha2::{Digest, Sha256};
use std::{fs::{File}, io::{Read, Write}, path::{Path, PathBuf}, collections::BTreeSet};
#[cfg(unix)]
use std::os::{fd::{AsRawFd, FromRawFd}, unix::fs::{MetadataExt}};

pub const MAX_BYTES: u64 = 16 * 1024 * 1024;
pub fn digest(bytes:&[u8])->String { format!("{:x}",Sha256::digest(bytes)) }

// serde_json normally discards duplicate object members. Reject them recursively.
struct Unique(Value);
impl<'de> Deserialize<'de> for Unique {
 fn deserialize<D:de::Deserializer<'de>>(d:D)->std::result::Result<Self,D::Error> {
  struct V;
  impl<'de> Visitor<'de> for V {
   type Value=Unique;
   fn expecting(&self,f:&mut std::fmt::Formatter)->std::fmt::Result { f.write_str("JSON without duplicate keys") }
   fn visit_map<A:MapAccess<'de>>(self,mut a:A)->std::result::Result<Unique,A::Error> { let mut m=Map::new(); while let Some((k,v))=a.next_entry::<String,Unique>()? { if m.insert(k,v.0).is_some() { return Err(de::Error::custom("DUPLICATE_JSON_KEY")); } } Ok(Unique(Value::Object(m))) }
   fn visit_seq<A:SeqAccess<'de>>(self,mut a:A)->std::result::Result<Unique,A::Error> { let mut v=Vec::new(); while let Some(x)=a.next_element::<Unique>()? { v.push(x.0); } Ok(Unique(Value::Array(v))) }
   fn visit_str<E:de::Error>(self,v:&str)->std::result::Result<Unique,E> { Ok(Unique(Value::String(v.into()))) }
   fn visit_string<E:de::Error>(self,v:String)->std::result::Result<Unique,E> { Ok(Unique(Value::String(v))) }
   fn visit_bool<E:de::Error>(self,v:bool)->std::result::Result<Unique,E> { Ok(Unique(Value::Bool(v))) }
   fn visit_u64<E:de::Error>(self,v:u64)->std::result::Result<Unique,E> { Ok(Unique(v.into())) }
   fn visit_i64<E:de::Error>(self,v:i64)->std::result::Result<Unique,E> { Ok(Unique(v.into())) }
   fn visit_f64<E:de::Error>(self,v:f64)->std::result::Result<Unique,E> { serde_json::Number::from_f64(v).map(|n| Unique(Value::Number(n))).ok_or_else(||de::Error::custom("invalid number")) }
   fn visit_unit<E:de::Error>(self)->std::result::Result<Unique,E> { Ok(Unique(Value::Null)) }
  }
  d.deserialize_any(V)
 }
}
pub fn json(bytes:&[u8])->Result<Value> { let mut d=serde_json::Deserializer::from_slice(bytes); let v=Unique::deserialize(&mut d)?; d.end()?; Ok(v.0) }

#[cfg(unix)]
fn directory(path:&Path)->Result<File> {
 use std::path::Component;
 ensure!(path.is_absolute(),"ARTIFACT_UNSAFE: absolute path required");
 let mut dir=File::open("/")?;
 for c in path.components() {
  match c { Component::RootDir=>(), Component::Normal(name)=> {
   let name=std::ffi::CString::new(name.as_encoded_bytes())?;
   // SAFETY: dir and name remain live; successful openat transfers one fd.
   let fd=unsafe { libc::openat(dir.as_raw_fd(),name.as_ptr(),libc::O_RDONLY|libc::O_DIRECTORY|libc::O_NOFOLLOW|libc::O_CLOEXEC) };
   ensure!(fd>=0,"ARTIFACT_UNSAFE: cannot open directory: {}",std::io::Error::last_os_error());
   dir=unsafe { File::from_raw_fd(fd) };
  }, _=>bail!("ARTIFACT_UNSAFE: traversal") }
 }
 let m=dir.metadata()?;
 ensure!(m.uid()==unsafe{libc::geteuid()} && m.mode()&0o077==0,"ARTIFACT_UNSAFE: parent must be owned and private");
 Ok(dir)
}
#[cfg(unix)]
fn open(path:&Path,create:bool)->Result<File> {
 let absolute=if path.is_absolute(){path.to_owned()}else{std::env::current_dir()?.join(path)};
 let parent=directory(absolute.parent().context("ARTIFACT_UNSAFE: no parent")?)?;
 let name=std::ffi::CString::new(absolute.file_name().context("ARTIFACT_UNSAFE: no filename")?.as_encoded_bytes())?;
 let flags=if create { libc::O_WRONLY|libc::O_CREAT|libc::O_EXCL } else { libc::O_RDONLY };
 let fd=unsafe{libc::openat(parent.as_raw_fd(),name.as_ptr(),flags|libc::O_NOFOLLOW|libc::O_NONBLOCK|libc::O_CLOEXEC,0o600)};
 ensure!(fd>=0,"ARTIFACT_UNSAFE: cannot open file: {}",std::io::Error::last_os_error());
 let file=unsafe{File::from_raw_fd(fd)}; let m=file.metadata()?;
 ensure!(m.is_file() && m.uid()==unsafe{libc::geteuid()} && m.nlink()==1 && m.mode()&0o077==0,"ARTIFACT_UNSAFE: file must be private, owned, regular and singly linked");
 Ok(file)
}
#[cfg(not(unix))]
fn open(_path:&Path,_create:bool)->Result<File> { bail!("ARTIFACT_UNSAFE: descriptor validation requires Unix") }
pub fn read(path:&Path)->Result<Vec<u8>> { let f=open(path,false)?; ensure!(f.metadata()?.len()<=MAX_BYTES,"EXECUTION_LIMIT_REACHED: artifact size"); let mut bytes=Vec::new(); f.take(MAX_BYTES+1).read_to_end(&mut bytes)?; ensure!(bytes.len() as u64<=MAX_BYTES,"EXECUTION_LIMIT_REACHED: artifact size"); Ok(bytes) }
pub fn write(path:&Path,bytes:&[u8])->Result<()> {
 ensure!(bytes.len() as u64<=MAX_BYTES,"RESULT_PERSIST_FAILED: size");
 // O_EXCL publishes an immutable name. Readers only receive it after sync_all.
 let mut f=open(path,true)?; f.write_all(bytes).context("RESULT_PERSIST_FAILED")?; f.sync_all().context("RESULT_PERSIST_FAILED")?; Ok(())
}
#[derive(Debug,Clone,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Authorization { pub schema_version:u32, pub run_id:String, pub target:Target, pub no_merge:bool, pub criteria:Vec<String> }
pub struct Store { root:PathBuf, pub authorization:Authorization }
impl Store {
 /// Launch-only entry point. This is intentionally not a request field.
 pub fn initialize(root:&Path, authorization:Authorization)->Result<Self> {
  ensure!(authorization.schema_version==1 && identifier(&authorization.run_id),"INVALID_REQUEST: launch identity");
  if !root.exists() {
   #[cfg(unix)] { use std::os::unix::fs::DirBuilderExt; std::fs::DirBuilder::new().mode(0o700).create(root)?; }
  }
  write(&root.join("authorization.json"),&serde_json::to_vec(&authorization)?)?;
  Ok(Self{root:root.into(),authorization})
 }
 pub fn load(root:&Path,request:&Request)->Result<Self> {
  let a:Authorization=serde_json::from_value(json(&read(&root.join("authorization.json"))?)?)?;
  ensure!(a.schema_version==1,"UNSUPPORTED_SCHEMA");
  ensure!(a.run_id==request.run_id,"CROSS_RUN_REPLAY"); ensure!(a.target==request.target,"TARGET_MISMATCH");
  ensure!(request.resume_ref.is_none(),"RESUME_BINDING_INVALID: cross-run imports are not authorized");
  let s=Self{root:root.into(),authorization:a};
  if let Some(r)=&request.predecessor_ref { let _=s.record(r,request,None,false)?; }
  Ok(s)
 }
 pub fn put(&self,kind:&str,bytes:&[u8])->Result<ArtifactRef> {
  ensure!(identifier(kind),"INVALID_REQUEST: artifact kind");
  let id=format!("{}-{}",kind,uuid::Uuid::new_v4().simple()); let r=ArtifactRef{artifact_id:id,sha256:digest(bytes)};
  write(&self.root.join(&r.artifact_id),bytes)?;
  // Separate registration from bytes, including the tool-assigned kind.
  write(&self.root.join(format!("{}.registration",r.artifact_id)),&serde_json::to_vec(&(kind,&r))?)?; Ok(r)
 }
 pub fn get(&self,r:&ArtifactRef,kind:Option<&str>)->Result<Vec<u8>> {
  ensure!(reference(r),"INVALID_REQUEST: artifact reference");
  let registration=read(&self.root.join(format!("{}.registration",r.artifact_id))).context("ARTIFACT_UNREGISTERED")?;
  let (actual,registered):(String,ArtifactRef)=serde_json::from_value(json(&registration)?)?;
  ensure!(&registered==r && kind.is_none_or(|k| k==actual),"ARTIFACT_UNREGISTERED: reference kind or registration differs");
  let bytes=read(&self.root.join(&r.artifact_id))?;
  ensure!(digest(&bytes)==r.sha256,"ARTIFACT_HASH_MISMATCH"); Ok(bytes)
 }
 pub fn register(&self,record:&Record)->Result<ArtifactRef> { self.put(&record.kind,&serde_json::to_vec(record)?) }
 pub fn record(&self,r:&ArtifactRef,request:&Request,kind:Option<&str>,same_round:bool)->Result<Record> {
  let v:Record=serde_json::from_value(json(&self.get(r,kind)?)?)?;
  ensure!(v.schema_version==1,"UNSUPPORTED_SCHEMA"); ensure!(v.run_id==request.run_id,"CROSS_RUN_REPLAY");
  ensure!(v.target==request.target,"TARGET_MISMATCH"); ensure!(v.revision==request.revision,"REVISION_MISMATCH");
  ensure!(!same_round || v.round_id==request.round_id,"PREDECESSOR_MISMATCH: stale round"); Ok(v)
 }
 pub fn lock(&self)->Result<File> {
  let path=self.root.join("merge.lock");
  if !path.exists() { match write(&path,b"") { Ok(())=>(), Err(e) if path.exists()=>{tracing::debug!("lock created concurrently: {e}");}, Err(e)=>return Err(e) } }
  let f=open(&path,false)?; fs2::FileExt::try_lock_exclusive(&f).context("ATTEMPT_IN_PROGRESS")?; Ok(f)
 }
 pub fn root(&self)->&Path { &self.root }
}
pub fn unique_ids<'a>(ids:impl Iterator<Item=&'a str>)->bool { let mut seen=BTreeSet::new(); ids.into_iter().all(|id|seen.insert(id)) }
