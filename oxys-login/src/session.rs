use super::*;

pub(super) fn authenticate(username: &str, password: &str) -> Result<(), String> {
    const SERVICES: &[&str] = &["login", "system-auth"];

    let mut last_error = "failed to initialize PAM".to_string();
    for service in SERVICES {
        let mut client = match Client::with_password(service) {
            Ok(client) => client,
            Err(error) => {
                last_error = format!("{error:?}");
                continue;
            }
        };

        client
            .conversation_mut()
            .set_credentials(username.to_string(), password.to_string());

        match client.authenticate() {
            Ok(()) => return Ok(()),
            Err(error) => {
                last_error = format!("{error:?}");
            }
        }
    }

    Err(last_error)
}

pub(super) fn exec_tty_login() -> Result<(), Box<dyn std::error::Error>> {
    // oxys-login runs as agetty's login program on tty1. Replacing ourselves
    // with the standard login program gives the operator a normal diagnostic
    // shell; after logout, init respawns agetty and oxys-login appears again.
    let error = Command::new("/bin/login").exec();
    Err(format!("failed to exec /bin/login: {error}").into())
}

pub(super) fn exec_niri_session(
    username: &str,
    password: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let user = get_user_by_name(username)
        .ok_or_else(|| format!("unable to resolve user record for {username}"))?;

    become_session_leader()?;

    let shell = user.shell().to_string_lossy().to_string();
    let home = user.home_dir().to_string_lossy().to_string();
    let uid = user.uid();
    let gid = user.primary_group_id();
    let mut pam_session = PamSession::open(username, password)?;
    let environment = session_environment(&mut pam_session, username, uid, &home, &shell);
    let initgroups_user = CString::new(username)?;

    // niri's own `niri-session` script drives `systemctl --user`, which does not
    // exist on OpenRC/elogind, so it stalls right after niri loads its config.
    // Launch niri the way the OpenRC niri ebuild rewrites the .desktop entry to:
    // a private session bus (dbus-run-session) wrapping `niri --session`.
    //
    // Redirect niri's stdout+stderr to ~/niri.log so a hung or failed session
    // can be inspected after the fact (e.g. over SSH: `less ~/niri.log`).
    // Execing niri directly leaves no trace, and a wedged compositor holds tty1
    // in graphics mode so its console output can't be scrolled back or even
    // switched away from. `exec` keeps the tree as dbus-run-session -> niri, and
    // RUST_BACKTRACE surfaces a panic location if niri aborts.
    let session_cmd = "exec niri --session >\"${HOME}/niri.log\" 2>&1";
    let _ = unsafe {
        Command::new("dbus-run-session")
            .args(["--", "sh", "-c", session_cmd])
            .env_clear()
            .envs(environment)
            .env("RUST_BACKTRACE", "1")
            .current_dir(&home)
            .pre_exec(move || {
                if libc::initgroups(initgroups_user.as_ptr(), gid) != 0 {
                    return Err(io::Error::last_os_error());
                }
                if libc::setgid(gid) != 0 {
                    return Err(io::Error::last_os_error());
                }
                if libc::setuid(uid) != 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            })
            .exec()
    };

    drop(pam_session);
    Err("failed to exec dbus-run-session -- niri --session".into())
}

struct PamCredentials {
    username: CString,
    password: CString,
}

struct PamSession {
    handle: *mut pam_ffi::pam_handle_t,
    _credentials: Box<PamCredentials>,
    credentials_established: bool,
    session_open: bool,
}

impl PamSession {
    fn open(username: &str, password: &str) -> Result<Self, String> {
        const SERVICES: &[&str] = &["login", "system-auth"];

        let mut last_error = "failed to initialize PAM".to_string();
        for service in SERVICES {
            match Self::open_with_service(service, username, password) {
                Ok(session) => return Ok(session),
                Err(error) => last_error = error,
            }
        }

        Err(last_error)
    }

    fn open_with_service(service: &str, username: &str, password: &str) -> Result<Self, String> {
        let service = CString::new(service).map_err(|_| "PAM service contains NUL".to_string())?;
        let mut credentials = Box::new(PamCredentials {
            username: CString::new(username).map_err(|_| "username contains NUL".to_string())?,
            password: CString::new(password).map_err(|_| "password contains NUL".to_string())?,
        });
        let conversation = pam_ffi::pam_conv {
            conv: Some(pam_conversation),
            appdata_ptr: credentials.as_mut() as *mut PamCredentials as *mut c_void,
        };

        let mut handle = std::ptr::null_mut();
        let start_code = unsafe {
            pam_ffi::pam_start(
                service.as_ptr(),
                credentials.username.as_ptr(),
                &conversation,
                &mut handle,
            )
        };
        if start_code != pam_ffi::PAM_SUCCESS {
            return Err(format!("pam_start failed: {start_code}"));
        }

        let mut session = Self {
            handle,
            _credentials: credentials,
            credentials_established: false,
            session_open: false,
        };

        session.set_tty();
        session.check(
            unsafe { pam_ffi::pam_authenticate(session.handle, 0) },
            "authenticate",
        )?;
        session.check(
            unsafe { pam_ffi::pam_acct_mgmt(session.handle, 0) },
            "account",
        )?;
        session.check(
            unsafe { pam_ffi::pam_setcred(session.handle, pam_ffi::PAM_ESTABLISH_CRED) },
            "set credentials",
        )?;
        session.credentials_established = true;
        session.check(
            unsafe { pam_ffi::pam_open_session(session.handle, 0) },
            "open session",
        )?;
        session.session_open = true;
        session.check(
            unsafe { pam_ffi::pam_setcred(session.handle, pam_ffi::PAM_REINITIALIZE_CRED) },
            "refresh credentials",
        )?;

        Ok(session)
    }

    fn environment(&mut self) -> Vec<(String, String)> {
        let env = unsafe { pam_ffi::pam_getenvlist(self.handle) };
        if env.is_null() {
            return Vec::new();
        }

        let mut result = Vec::new();
        unsafe {
            let mut current = env;
            while !(*current).is_null() {
                let raw = CStr::from_ptr(*current).to_string_lossy();
                if let Some((key, value)) = raw.split_once('=') {
                    if !key.is_empty() {
                        result.push((key.to_string(), value.to_string()));
                    }
                }
                current = current.add(1);
            }
            pam_ffi::pam_misc_drop_env(env);
        }
        result
    }

    fn check(&mut self, code: c_int, action: &str) -> Result<(), String> {
        if code == pam_ffi::PAM_SUCCESS {
            return Ok(());
        }

        let message = unsafe {
            let ptr = pam_ffi::pam_strerror(self.handle, code);
            if ptr.is_null() {
                format!("PAM error {code}")
            } else {
                CStr::from_ptr(ptr).to_string_lossy().into_owned()
            }
        };
        Err(format!("PAM {action} failed: {message}"))
    }

    fn set_tty(&mut self) {
        let Some(tty) = current_tty() else {
            return;
        };
        let Ok(tty) = CString::new(tty) else {
            return;
        };
        unsafe {
            pam_ffi::pam_set_item(self.handle, pam_ffi::PAM_TTY, tty.as_ptr() as *const c_void);
        }
    }
}

impl Drop for PamSession {
    fn drop(&mut self) {
        let mut status = pam_ffi::PAM_SUCCESS;
        if self.session_open {
            status = unsafe { pam_ffi::pam_close_session(self.handle, 0) };
        }
        if self.credentials_established {
            status = unsafe { pam_ffi::pam_setcred(self.handle, pam_ffi::PAM_DELETE_CRED) };
        }
        unsafe {
            pam_ffi::pam_end(self.handle, status);
        }
    }
}

unsafe extern "C" fn pam_conversation(
    num_msg: c_int,
    msg: *mut *const pam_ffi::pam_message,
    out_resp: *mut *mut pam_ffi::pam_response,
    appdata_ptr: *mut c_void,
) -> c_int {
    unsafe {
        if num_msg <= 0 || num_msg > pam_ffi::PAM_MAX_NUM_MSG || msg.is_null() || out_resp.is_null()
        {
            return pam_ffi::PAM_CONV_ERR;
        }
        let credentials = &*(appdata_ptr as *const PamCredentials);
        let responses = libc::calloc(
            num_msg as usize,
            std::mem::size_of::<pam_ffi::pam_response>(),
        ) as *mut pam_ffi::pam_response;
        if responses.is_null() {
            return pam_ffi::PAM_BUF_ERR;
        }

        for index in 0..num_msg as isize {
            let message = *msg.offset(index);
            if message.is_null() {
                free_pam_responses(responses, index);
                return pam_ffi::PAM_CONV_ERR;
            }

            let response = &mut *responses.offset(index);
            let answer = match (*message).msg_style {
                pam_ffi::PAM_PROMPT_ECHO_ON => Some(credentials.username.as_ptr()),
                pam_ffi::PAM_PROMPT_ECHO_OFF => Some(credentials.password.as_ptr()),
                pam_ffi::PAM_TEXT_INFO | pam_ffi::PAM_ERROR_MSG => None,
                _ => {
                    free_pam_responses(responses, index);
                    return pam_ffi::PAM_CONV_ERR;
                }
            };

            if let Some(answer) = answer {
                response.resp = libc::strdup(answer);
                if response.resp.is_null() {
                    free_pam_responses(responses, index);
                    return pam_ffi::PAM_BUF_ERR;
                }
            }
        }

        *out_resp = responses;
        pam_ffi::PAM_SUCCESS
    }
}

unsafe fn free_pam_responses(responses: *mut pam_ffi::pam_response, initialized: isize) {
    unsafe {
        for index in 0..initialized {
            let response = &mut *responses.offset(index);
            if !response.resp.is_null() {
                libc::free(response.resp as *mut c_void);
            }
        }
        libc::free(responses as *mut c_void);
    }
}

fn session_environment(
    pam_session: &mut PamSession,
    username: &str,
    uid: u32,
    home: &str,
    shell: &str,
) -> Vec<(String, String)> {
    let mut environment = pam_session.environment();
    set_env_default(&mut environment, "USER", username);
    set_env_default(&mut environment, "LOGNAME", username);
    set_env_default(&mut environment, "HOME", home);
    set_env_default(&mut environment, "PWD", home);
    set_env_default(&mut environment, "SHELL", shell);
    set_env_default(
        &mut environment,
        "XDG_RUNTIME_DIR",
        &format!("/run/user/{uid}"),
    );
    set_env_default(&mut environment, "XDG_SESSION_TYPE", "wayland");
    set_env_default(&mut environment, "XDG_SESSION_CLASS", "user");
    for (key, value) in session_config() {
        if key != "OXYS_FALLBACK_TTY_LOGIN" {
            set_env_value(&mut environment, &key, &value);
        }
    }
    set_env_default(
        &mut environment,
        "PATH",
        "/usr/local/sbin:/usr/local/bin:/usr/bin:/bin",
    );
    environment
}

fn session_config() -> Vec<(String, String)> {
    std::fs::read_to_string("/etc/oxys/session.env")
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let (key, value) = line.split_once('=')?;
            Some((key.trim().to_owned(), value.trim().to_owned()))
        })
        .collect()
}

pub(super) fn session_config_value(key: &str) -> Option<String> {
    session_config()
        .into_iter()
        .find_map(|(name, value)| (name == key).then_some(value))
}

fn set_env_value(environment: &mut Vec<(String, String)>, key: &str, value: &str) {
    if let Some((_, existing)) = environment
        .iter_mut()
        .find(|(existing_key, _)| existing_key == key)
    {
        *existing = value.to_owned();
    } else {
        environment.push((key.to_owned(), value.to_owned()));
    }
}

fn set_env_default(environment: &mut Vec<(String, String)>, key: &str, value: &str) {
    if let Some((_, existing)) = environment
        .iter_mut()
        .find(|(existing_key, _)| existing_key == key)
    {
        if existing.trim().is_empty() {
            *existing = value.to_string();
        }
        return;
    }

    environment.push((key.to_string(), value.to_string()));
}

fn become_session_leader() -> io::Result<()> {
    if unsafe { libc::setsid() } == -1 {
        if unsafe { libc::getsid(0) } == unsafe { libc::getpid() } {
            return Ok(());
        }
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn current_tty() -> Option<String> {
    fs::read_link("/proc/self/fd/0")
        .ok()
        .map(|path| path.display().to_string())
        .filter(|path| path.starts_with("/dev/"))
}
