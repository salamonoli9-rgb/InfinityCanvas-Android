//! Android photo library through JNI: MediaStore listing, thumbnails and file reading.
//!
//! The app has no Java code (NativeActivity), so the system photo picker (which returns its
//! result to an Activity callback) can't be used; instead photos are listed from MediaStore
//! after the READ_MEDIA_IMAGES permission is granted.

use crate::photos::{PhotoBackend, PhotoEntry};
use image::RgbaImage;
use jni::objects::{GlobalRef, JObject, JValue};
use jni::{JNIEnv, JavaVM};
use std::io::Read;
use std::os::fd::FromRawFd;

const PERMISSIONS: [&str; 2] = ["android.permission.READ_MEDIA_IMAGES", "android.permission.READ_MEDIA_VISUAL_USER_SELECTED"];
const LEGACY_PERMISSION: &str = "android.permission.READ_EXTERNAL_STORAGE";
const MAX_PHOTOS: usize = 400;

pub struct MediaStorePhotos {
    vm: JavaVM,
    activity: GlobalRef,
    sdk: i32,
}

type JResult<T> = jni::errors::Result<T>;

impl MediaStorePhotos {
    /// # Safety
    /// `vm` and `activity` must be the process JavaVM and the NativeActivity object.
    pub unsafe fn new(vm: *mut std::ffi::c_void, activity: *mut std::ffi::c_void) -> Option<Self> {
        let vm = unsafe { JavaVM::from_raw(vm as *mut jni::sys::JavaVM) }.ok()?;
        let activity = {
            let env = vm.attach_current_thread().ok()?;
            let obj = unsafe { JObject::from_raw(activity as jni::sys::jobject) };
            env.new_global_ref(obj).ok()?
        };
        let sdk = {
            let mut env = vm.attach_current_thread().ok()?;
            env.get_static_field("android/os/Build$VERSION", "SDK_INT", "I").and_then(|v| v.i()).unwrap_or(26)
        };
        Some(Self { vm, activity, sdk })
    }

    fn permissions(&self) -> Vec<&'static str> {
        if self.sdk >= 33 { PERMISSIONS.to_vec() } else { vec![LEGACY_PERMISSION] }
    }

    fn granted(&self, env: &mut JNIEnv) -> JResult<bool> {
        for p in self.permissions() {
            let s = env.new_string(p)?;
            let r = env.call_method(self.activity.as_obj(), "checkSelfPermission", "(Ljava/lang/String;)I", &[JValue::Object(&s)])?.i()?;
            if r == 0 {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn resolver<'l>(&self, env: &mut JNIEnv<'l>) -> JResult<JObject<'l>> {
        env.call_method(self.activity.as_obj(), "getContentResolver", "()Landroid/content/ContentResolver;", &[])?.l()
    }

    fn images_uri<'l>(env: &mut JNIEnv<'l>) -> JResult<JObject<'l>> {
        env.get_static_field("android/provider/MediaStore$Images$Media", "EXTERNAL_CONTENT_URI", "Landroid/net/Uri;")?.l()
    }

    fn uri_for<'l>(env: &mut JNIEnv<'l>, key: &str) -> JResult<JObject<'l>> {
        let id: i64 = key.parse().unwrap_or(-1);
        let base = Self::images_uri(env)?;
        env.call_static_method(
            "android/content/ContentUris",
            "withAppendedId",
            "(Landroid/net/Uri;J)Landroid/net/Uri;",
            &[JValue::Object(&base), JValue::Long(id)],
        )?
        .l()
    }

    fn list_inner(&self, env: &mut JNIEnv) -> JResult<Vec<PhotoEntry>> {
        let resolver = self.resolver(env)?;
        let uri = Self::images_uri(env)?;
        let projection = env.new_object_array(2, "java/lang/String", JObject::null())?;
        let id_col = env.new_string("_id")?;
        let name_col = env.new_string("_display_name")?;
        env.set_object_array_element(&projection, 0, &id_col)?;
        env.set_object_array_element(&projection, 1, &name_col)?;
        let order = env.new_string("date_modified DESC")?;
        let cursor = env
            .call_method(
                &resolver,
                "query",
                "(Landroid/net/Uri;[Ljava/lang/String;Ljava/lang/String;[Ljava/lang/String;Ljava/lang/String;)Landroid/database/Cursor;",
                &[JValue::Object(&uri), JValue::Object(&projection), JValue::Object(&JObject::null()), JValue::Object(&JObject::null()), JValue::Object(&order)],
            )?
            .l()?;
        let mut out = Vec::new();
        if cursor.is_null() {
            return Ok(out);
        }
        while out.len() < MAX_PHOTOS && env.call_method(&cursor, "moveToNext", "()Z", &[])?.z()? {
            let id = env.call_method(&cursor, "getLong", "(I)J", &[JValue::Int(0)])?.j()?;
            let name_obj = env.call_method(&cursor, "getString", "(I)Ljava/lang/String;", &[JValue::Int(1)])?.l()?;
            let name = if name_obj.is_null() { String::new() } else { env.get_string(&name_obj.into())?.into() };
            out.push(PhotoEntry { key: id.to_string(), name });
        }
        env.call_method(&cursor, "close", "()V", &[])?;
        Ok(out)
    }

    fn read_inner(&self, env: &mut JNIEnv, key: &str) -> JResult<Option<Vec<u8>>> {
        let resolver = self.resolver(env)?;
        let uri = Self::uri_for(env, key)?;
        let mode = env.new_string("r")?;
        let pfd = env
            .call_method(
                &resolver,
                "openFileDescriptor",
                "(Landroid/net/Uri;Ljava/lang/String;)Landroid/os/ParcelFileDescriptor;",
                &[JValue::Object(&uri), JValue::Object(&mode)],
            )?
            .l()?;
        if pfd.is_null() {
            return Ok(None);
        }
        let fd = env.call_method(&pfd, "detachFd", "()I", &[])?.i()?;
        // SAFETY: detachFd hands over ownership of a valid descriptor.
        let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
        let mut bytes = Vec::new();
        Ok(file.read_to_end(&mut bytes).ok().map(|_| bytes))
    }

    fn thumbnail_inner(&self, env: &mut JNIEnv, key: &str, side: u32) -> JResult<Option<RgbaImage>> {
        let resolver = self.resolver(env)?;
        let uri = Self::uri_for(env, key)?;
        let size = env.new_object("android/util/Size", "(II)V", &[JValue::Int(side as i32), JValue::Int(side as i32)])?;
        let bitmap = env
            .call_method(
                &resolver,
                "loadThumbnail",
                "(Landroid/net/Uri;Landroid/util/Size;Landroid/os/CancellationSignal;)Landroid/graphics/Bitmap;",
                &[JValue::Object(&uri), JValue::Object(&size), JValue::Object(&JObject::null())],
            )?
            .l()?;
        if bitmap.is_null() {
            return Ok(None);
        }
        let w = env.call_method(&bitmap, "getWidth", "()I", &[])?.i()?;
        let h = env.call_method(&bitmap, "getHeight", "()I", &[])?.i()?;
        if w <= 0 || h <= 0 {
            return Ok(None);
        }
        // getPixels converts any bitmap config to ARGB ints.
        let arr = env.new_int_array(w * h)?;
        env.call_method(
            &bitmap,
            "getPixels",
            "([IIIIIII)V",
            &[JValue::Object(&arr), JValue::Int(0), JValue::Int(w), JValue::Int(0), JValue::Int(0), JValue::Int(w), JValue::Int(h)],
        )?;
        let mut argb = vec![0i32; (w * h) as usize];
        env.get_int_array_region(&arr, 0, &mut argb)?;
        let _ = env.call_method(&bitmap, "recycle", "()V", &[]);
        let mut rgba = Vec::with_capacity(argb.len() * 4);
        for p in argb {
            let p = p as u32;
            rgba.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8, (p >> 24) as u8]);
        }
        Ok(RgbaImage::from_raw(w as u32, h as u32, rgba))
    }

    /// Inserts a PNG into MediaStore under Pictures/<EXPORT_FOLDER> (no permission needed for
    /// the app's own files on Android 10+).
    fn save_inner(&self, env: &mut JNIEnv, name: &str, png: &[u8]) -> JResult<Option<String>> {
        let resolver = self.resolver(env)?;
        let values = env.new_object("android/content/ContentValues", "()V", &[])?;
        let put = |env: &mut JNIEnv, k: &str, v: &str| -> JResult<()> {
            let k = env.new_string(k)?;
            let v = env.new_string(v)?;
            env.call_method(&values, "put", "(Ljava/lang/String;Ljava/lang/String;)V", &[JValue::Object(&k), JValue::Object(&v)])?;
            Ok(())
        };
        put(env, "_display_name", &format!("{name}.png"))?;
        put(env, "mime_type", "image/png")?;
        let folder = format!("Pictures/{}", crate::photos::EXPORT_FOLDER);
        if self.sdk >= 29 {
            put(env, "relative_path", &folder)?;
        }
        let uri = Self::images_uri(env)?;
        let item = env
            .call_method(&resolver, "insert", "(Landroid/net/Uri;Landroid/content/ContentValues;)Landroid/net/Uri;", &[JValue::Object(&uri), JValue::Object(&values)])?
            .l()?;
        if item.is_null() {
            return Ok(None);
        }
        let stream = env.call_method(&resolver, "openOutputStream", "(Landroid/net/Uri;)Ljava/io/OutputStream;", &[JValue::Object(&item)])?.l()?;
        if stream.is_null() {
            return Ok(None);
        }
        for chunk in png.chunks(1 << 20) {
            let arr = env.byte_array_from_slice(chunk)?;
            env.call_method(&stream, "write", "([B)V", &[JValue::Object(&arr)])?;
            env.delete_local_ref(arr)?;
        }
        env.call_method(&stream, "close", "()V", &[])?;
        Ok(Some(crate::i18n::tf("Galeria → {}", &[&folder])))
    }

    /// Runs `f` with an attached env; Java exceptions become `None`.
    fn with_env<T>(&self, f: impl FnOnce(&Self, &mut JNIEnv) -> JResult<T>) -> Option<T> {
        let mut env = self.vm.attach_current_thread().ok()?;
        let r = env.with_local_frame(64, |env| f(self, env));
        match r {
            Ok(v) => Some(v),
            Err(e) => {
                if env.exception_check().unwrap_or(false) {
                    let _ = env.exception_describe();
                    let _ = env.exception_clear();
                }
                log::warn!("photo library: {e}");
                None
            }
        }
    }
}

impl PhotoBackend for MediaStorePhotos {
    fn list(&mut self) -> Result<Vec<PhotoEntry>, String> {
        match self.with_env(|s, env| s.granted(env)) {
            Some(true) => {}
            _ => return Err("Aplikacja potrzebuje dostępu do zdjęć.".into()),
        }
        match self.with_env(|s, env| s.list_inner(env)) {
            Some(v) if !v.is_empty() => Ok(v),
            Some(_) => Err("Nie znaleziono zdjęć w galerii.".into()),
            None => Err("Nie udało się odczytać galerii zdjęć.".into()),
        }
    }

    fn read(&mut self, key: &str) -> Option<Vec<u8>> {
        self.with_env(|s, env| s.read_inner(env, key)).flatten()
    }

    fn thumbnail(&mut self, key: &str, side: u32) -> Option<RgbaImage> {
        self.with_env(|s, env| s.thumbnail_inner(env, key, side)).flatten()
    }

    fn save_png(&mut self, name: &str, png: &[u8]) -> Result<String, String> {
        self.with_env(|s, env| s.save_inner(env, name, png)).flatten().ok_or_else(|| "Nie udało się zapisać obrazu".to_string())
    }

    fn request_access(&mut self) {
        let perms = self.permissions();
        self.with_env(|s, env| {
            let arr = env.new_object_array(perms.len() as i32, "java/lang/String", JObject::null())?;
            for (i, p) in perms.iter().enumerate() {
                let js = env.new_string(p)?;
                env.set_object_array_element(&arr, i as i32, &js)?;
            }
            env.call_method(s.activity.as_obj(), "requestPermissions", "([Ljava/lang/String;I)V", &[JValue::Object(&arr), JValue::Int(7)])?;
            Ok(())
        });
    }
}

/// Video encoding with the phone's hardware H.264 encoder (MediaCodec + MediaMuxer), the
/// finished file copied into the gallery (Movies/<EXPORT_FOLDER>).
pub fn video_factory(vm: *mut std::ffi::c_void, activity: *mut std::ffi::c_void) -> crate::video::VideoFactory {
    let (vm, activity) = (vm as usize, activity as usize);
    std::sync::Arc::new(move |name: &str, w: u32, h: u32, fps: u32| -> Result<Box<dyn crate::video::VideoSink>, String> {
        // SAFETY: process-wide JavaVM and the NativeActivity object, alive while the app runs.
        let vm = unsafe { JavaVM::from_raw(vm as *mut jni::sys::JavaVM) }.map_err(|e| e.to_string())?;
        let sink = MediaCodecSink::new(vm, activity, name, w, h, fps).map_err(|e| format!("koder wideo: {e}"))?;
        Ok(Box::new(sink))
    })
}

struct MediaCodecSink {
    vm: JavaVM,
    activity: GlobalRef,
    codec: GlobalRef,
    muxer: GlobalRef,
    info: GlobalRef,
    track: Option<i32>,
    frame: u64,
    fps: u32,
    size: [u32; 2],
    path: std::path::PathBuf,
    name: String,
}

const BUFFER_FLAG_CODEC_CONFIG: i32 = 2;
const BUFFER_FLAG_END_OF_STREAM: i32 = 4;
const INFO_TRY_AGAIN_LATER: i32 = -1;
const INFO_OUTPUT_FORMAT_CHANGED: i32 = -2;
/// COLOR_FormatYUV420SemiPlanar (NV12), accepted by practically every hardware encoder.
const COLOR_FORMAT_NV12: i32 = 21;

/// JNI result with a Java exception turned into its message.
fn jerr(env: &mut JNIEnv, e: jni::errors::Error) -> String {
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
    e.to_string()
}

impl MediaCodecSink {
    fn new(vm: JavaVM, activity: usize, name: &str, w: u32, h: u32, fps: u32) -> Result<Self, String> {
        let mut env = vm.attach_current_thread_permanently().map_err(|e| e.to_string())?;
        let env = &mut env;
        let r = (|| -> JResult<_> {
            let activity = {
                // SAFETY: the NativeActivity jobject handed over by android-activity.
                let obj = unsafe { JObject::from_raw(activity as jni::sys::jobject) };
                env.new_global_ref(obj)?
            };
            let dir = env.call_method(activity.as_obj(), "getCacheDir", "()Ljava/io/File;", &[])?.l()?;
            let dir = env.call_method(&dir, "getAbsolutePath", "()Ljava/lang/String;", &[])?.l()?;
            let dir: String = env.get_string(&dir.into())?.into();
            let path = std::path::Path::new(&dir).join("export.mp4");
            let mime = env.new_string("video/avc")?;
            let format = env
                .call_static_method(
                    "android/media/MediaFormat",
                    "createVideoFormat",
                    "(Ljava/lang/String;II)Landroid/media/MediaFormat;",
                    &[JValue::Object(&mime), JValue::Int(w as i32), JValue::Int(h as i32)],
                )?
                .l()?;
            for (k, v) in [("color-format", COLOR_FORMAT_NV12), ("bitrate", (w * h * 5) as i32), ("frame-rate", fps as i32), ("i-frame-interval", 1)] {
                let k = env.new_string(k)?;
                env.call_method(&format, "setInteger", "(Ljava/lang/String;I)V", &[JValue::Object(&k), JValue::Int(v)])?;
            }
            let codec = env
                .call_static_method("android/media/MediaCodec", "createEncoderByType", "(Ljava/lang/String;)Landroid/media/MediaCodec;", &[JValue::Object(&mime)])?
                .l()?;
            env.call_method(
                &codec,
                "configure",
                "(Landroid/media/MediaFormat;Landroid/view/Surface;Landroid/media/MediaCrypto;I)V",
                &[JValue::Object(&format), JValue::Object(&JObject::null()), JValue::Object(&JObject::null()), JValue::Int(1)],
            )?;
            env.call_method(&codec, "start", "()V", &[])?;
            let jpath = env.new_string(path.to_string_lossy())?;
            let muxer = env.new_object("android/media/MediaMuxer", "(Ljava/lang/String;I)V", &[JValue::Object(&jpath), JValue::Int(0)])?;
            let info = env.new_object("android/media/MediaCodec$BufferInfo", "()V", &[])?;
            Ok((activity, env.new_global_ref(codec)?, env.new_global_ref(muxer)?, env.new_global_ref(info)?, path))
        })();
        let (activity, codec, muxer, info, path) = r.map_err(|e| jerr(env, e))?;
        Ok(Self { vm, activity, codec, muxer, info, track: None, frame: 0, fps, size: [w, h], path, name: name.to_string() })
    }

    /// Moves encoded output into the muxer. With `until_eos`, waits for the end of the stream.
    fn drain(&mut self, env: &mut JNIEnv, until_eos: bool) -> JResult<()> {
        let mut idle = 0;
        loop {
            let out = env
                .call_method(self.codec.as_obj(), "dequeueOutputBuffer", "(Landroid/media/MediaCodec$BufferInfo;J)I", &[JValue::Object(self.info.as_obj()), JValue::Long(if until_eos { 10_000 } else { 0 })])?
                .i()?;
            if out == INFO_TRY_AGAIN_LATER {
                idle += 1;
                if !until_eos || idle > 500 {
                    return Ok(());
                }
                continue;
            }
            if out == INFO_OUTPUT_FORMAT_CHANGED {
                let fmt = env.call_method(self.codec.as_obj(), "getOutputFormat", "()Landroid/media/MediaFormat;", &[])?.l()?;
                let t = env.call_method(self.muxer.as_obj(), "addTrack", "(Landroid/media/MediaFormat;)I", &[JValue::Object(&fmt)])?.i()?;
                env.call_method(self.muxer.as_obj(), "start", "()V", &[])?;
                self.track = Some(t);
                continue;
            }
            if out < 0 {
                continue;
            }
            let flags = env.get_field(self.info.as_obj(), "flags", "I")?.i()?;
            let size = env.get_field(self.info.as_obj(), "size", "I")?.i()?;
            if let Some(t) = self.track
                && flags & BUFFER_FLAG_CODEC_CONFIG == 0
                && size > 0
            {
                let buf = env.call_method(self.codec.as_obj(), "getOutputBuffer", "(I)Ljava/nio/ByteBuffer;", &[JValue::Int(out)])?.l()?;
                env.call_method(
                    self.muxer.as_obj(),
                    "writeSampleData",
                    "(ILjava/nio/ByteBuffer;Landroid/media/MediaCodec$BufferInfo;)V",
                    &[JValue::Int(t), JValue::Object(&buf), JValue::Object(self.info.as_obj())],
                )?;
                env.delete_local_ref(buf)?;
            }
            env.call_method(self.codec.as_obj(), "releaseOutputBuffer", "(IZ)V", &[JValue::Int(out), JValue::Bool(0)])?;
            if flags & BUFFER_FLAG_END_OF_STREAM != 0 {
                return Ok(());
            }
        }
    }

    /// Waits for a free input buffer (draining output meanwhile).
    fn input_buffer(&mut self, env: &mut JNIEnv) -> JResult<Option<i32>> {
        for _ in 0..400 {
            let idx = env.call_method(self.codec.as_obj(), "dequeueInputBuffer", "(J)I", &[JValue::Long(10_000)])?.i()?;
            if idx >= 0 {
                return Ok(Some(idx));
            }
            self.drain(env, false)?;
        }
        Ok(None)
    }

    fn pts(&self) -> i64 {
        (self.frame * 1_000_000 / self.fps as u64) as i64
    }

    fn copy_to_gallery(&self, env: &mut JNIEnv) -> JResult<String> {
        let bytes = std::fs::read(&self.path).unwrap_or_default();
        let resolver = env.call_method(self.activity.as_obj(), "getContentResolver", "()Landroid/content/ContentResolver;", &[])?.l()?;
        let values = env.new_object("android/content/ContentValues", "()V", &[])?;
        let folder = format!("Movies/{}", crate::photos::EXPORT_FOLDER);
        for (k, v) in [("_display_name", format!("{}.mp4", self.name)), ("mime_type", "video/mp4".into()), ("relative_path", folder.clone())] {
            let k = env.new_string(k)?;
            let v = env.new_string(v)?;
            env.call_method(&values, "put", "(Ljava/lang/String;Ljava/lang/String;)V", &[JValue::Object(&k), JValue::Object(&v)])?;
        }
        let uri = env.get_static_field("android/provider/MediaStore$Video$Media", "EXTERNAL_CONTENT_URI", "Landroid/net/Uri;")?.l()?;
        let item = env
            .call_method(&resolver, "insert", "(Landroid/net/Uri;Landroid/content/ContentValues;)Landroid/net/Uri;", &[JValue::Object(&uri), JValue::Object(&values)])?
            .l()?;
        let stream = env.call_method(&resolver, "openOutputStream", "(Landroid/net/Uri;)Ljava/io/OutputStream;", &[JValue::Object(&item)])?.l()?;
        for chunk in bytes.chunks(1 << 20) {
            let arr = env.byte_array_from_slice(chunk)?;
            env.call_method(&stream, "write", "([B)V", &[JValue::Object(&arr)])?;
            env.delete_local_ref(arr)?;
        }
        env.call_method(&stream, "close", "()V", &[])?;
        Ok(crate::i18n::tf("Galeria → {}", &[&folder]))
    }
}

impl crate::video::VideoSink for MediaCodecSink {
    fn push(&mut self, rgba: &[u8]) -> Result<(), String> {
        let nv12 = crate::video::rgba_to_nv12(rgba, self.size[0] as usize, self.size[1] as usize);
        let vm = unsafe { JavaVM::from_raw(self.vm.get_java_vm_pointer()) }.map_err(|e| e.to_string())?;
        let mut env = vm.attach_current_thread_permanently().map_err(|e| e.to_string())?;
        let env = &mut env;
        let r = env.with_local_frame(32, |env| -> JResult<bool> {
            let Some(idx) = self.input_buffer(env)? else { return Ok(false) };
            let buf = env.call_method(self.codec.as_obj(), "getInputBuffer", "(I)Ljava/nio/ByteBuffer;", &[JValue::Int(idx)])?.l()?;
            let buf = jni::objects::JByteBuffer::from(buf);
            let addr = env.get_direct_buffer_address(&buf)?;
            let cap = env.get_direct_buffer_capacity(&buf)?;
            let n = nv12.len().min(cap);
            // SAFETY: a direct buffer of `cap` bytes owned by the codec until queued.
            unsafe { std::ptr::copy_nonoverlapping(nv12.as_ptr(), addr, n) };
            let pts = self.pts();
            env.call_method(self.codec.as_obj(), "queueInputBuffer", "(IIIJI)V", &[JValue::Int(idx), JValue::Int(0), JValue::Int(n as i32), JValue::Long(pts), JValue::Int(0)])?;
            self.frame += 1;
            self.drain(env, false)?;
            Ok(true)
        });
        match r {
            Ok(true) => Ok(()),
            Ok(false) => Err("koder nie przyjmuje klatek".into()),
            Err(e) => Err(jerr(env, e)),
        }
    }

    fn finish(mut self: Box<Self>) -> Result<String, String> {
        let vm = unsafe { JavaVM::from_raw(self.vm.get_java_vm_pointer()) }.map_err(|e| e.to_string())?;
        let mut env = vm.attach_current_thread_permanently().map_err(|e| e.to_string())?;
        let env = &mut env;
        let r = env.with_local_frame(64, |env| -> JResult<String> {
            if let Some(idx) = self.input_buffer(env)? {
                let pts = self.pts();
                env.call_method(
                    self.codec.as_obj(),
                    "queueInputBuffer",
                    "(IIIJI)V",
                    &[JValue::Int(idx), JValue::Int(0), JValue::Int(0), JValue::Long(pts), JValue::Int(BUFFER_FLAG_END_OF_STREAM)],
                )?;
            }
            self.drain(env, true)?;
            env.call_method(self.codec.as_obj(), "stop", "()V", &[])?;
            env.call_method(self.codec.as_obj(), "release", "()V", &[])?;
            env.call_method(self.muxer.as_obj(), "stop", "()V", &[])?;
            env.call_method(self.muxer.as_obj(), "release", "()V", &[])?;
            self.copy_to_gallery(env)
        });
        let _ = std::fs::remove_file(&self.path);
        r.map_err(|e| jerr(env, e))
    }
}
