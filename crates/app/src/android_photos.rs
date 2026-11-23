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
        Ok(Some(format!("Galeria → {folder}")))
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
