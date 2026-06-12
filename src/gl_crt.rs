use sdl2::rect::Rect;
use sdl2::render::{Canvas, Texture};
use sdl2::video::Window;
use sdl2::VideoSubsystem;
use std::ffi::{CStr, CString};

type GLenum = u32;
type GLint = i32;
type GLuint = u32;
type GLsizei = i32;
type GLfloat = f32;
type GLchar = i8;

const GL_TEXTURE_2D: GLenum = 0x0DE1;
const GL_TEXTURE0: GLenum = 0x84C0;
const GL_ACTIVE_TEXTURE: GLenum = 0x84E0;
const GL_TEXTURE_BINDING_2D: GLenum = 0x8069;
const GL_TRIANGLE_STRIP: GLenum = 0x0005;
const GL_FRAGMENT_SHADER: GLenum = 0x8B30;
const GL_VERTEX_SHADER: GLenum = 0x8B31;
const GL_COMPILE_STATUS: GLenum = 0x8B81;
const GL_LINK_STATUS: GLenum = 0x8B82;
const GL_INFO_LOG_LENGTH: GLenum = 0x8B84;
const GL_VERSION: GLenum = 0x1F02;
const GL_DEPTH_TEST: GLenum = 0x0B71;
const GL_CULL_FACE: GLenum = 0x0B44;
const GL_BLEND: GLenum = 0x0BE2;
const GL_CURRENT_PROGRAM: GLenum = 0x8B8D;
const GL_VIEWPORT: GLenum = 0x0BA2;
const GL_TEXTURE_MIN_FILTER: GLenum = 0x2801;
const GL_TEXTURE_MAG_FILTER: GLenum = 0x2800;
const GL_TEXTURE_WRAP_S: GLenum = 0x2802;
const GL_TEXTURE_WRAP_T: GLenum = 0x2803;
const GL_LINEAR: GLint = 0x2601;
const GL_CLAMP_TO_EDGE: GLint = 0x812F;

pub struct GlCrtRenderer {
    gl: GlFns,
    program: GLuint,
    u_frame: GLint,
    u_tex_scale: GLint,
    u_frame_size: GLint,
    u_mode: GLint,
}

impl GlCrtRenderer {
    pub fn new(canvas: &Canvas<Window>) -> Result<Self, String> {
        if !canvas.info().name.eq_ignore_ascii_case("opengl") {
            return Err(format!(
                "CRT SHADER requires SDL opengl renderer, got {}",
                canvas.info().name
            ));
        }

        let gl = GlFns::load(canvas.window().subsystem())?;
        unsafe {
            let version = gl.get_string(GL_VERSION);
            if !version.is_null() {
                let version = CStr::from_ptr(version.cast()).to_string_lossy();
                println!("[render] CRT shader OpenGL version: {version}");
            }
        }

        let program = unsafe { compile_program(&gl, CRT_VERTEX_SHADER, CRT_FRAGMENT_SHADER)? };
        let u_frame = uniform_location(&gl, program, "u_frame")?;
        let u_tex_scale = uniform_location(&gl, program, "u_tex_scale")?;
        let u_frame_size = uniform_location(&gl, program, "u_frame_size")?;
        let u_mode = uniform_location(&gl, program, "u_mode")?;
        println!("[render] CRT shader initialized");

        Ok(Self {
            gl,
            program,
            u_frame,
            u_tex_scale,
            u_frame_size,
            u_mode,
        })
    }

    pub fn draw(
        &mut self,
        canvas: &mut Canvas<Window>,
        texture: &mut Texture<'_>,
        dst: Rect,
        frame_size: (u32, u32),
        output_size: (u32, u32),
        mode: i32,
    ) -> Result<(), String> {
        if dst.width() == 0 || dst.height() == 0 || output_size.0 == 0 || output_size.1 == 0 {
            return Ok(());
        }

        unsafe {
            let _ = sdl2::sys::SDL_RenderFlush(canvas.raw());
            let state = GlStateSnapshot::capture(&self.gl);
            self.gl.active_texture(GL_TEXTURE0);

            let mut tex_w = 0.0;
            let mut tex_h = 0.0;
            if sdl2::sys::SDL_GL_BindTexture(texture.raw(), &mut tex_w, &mut tex_h) != 0 {
                state.restore(&self.gl);
                return Err(sdl2::get_error());
            }

            self.gl.disable(GL_DEPTH_TEST);
            self.gl.disable(GL_CULL_FACE);
            self.gl.disable(GL_BLEND);
            self.gl.viewport(0, 0, output_size.0 as GLsizei, output_size.1 as GLsizei);
            self.gl.use_program(self.program);
            self.gl.active_texture(GL_TEXTURE0);
            self.gl.uniform_1i(self.u_frame, 0);
            self.gl.uniform_1i(self.u_mode, mode);
            self.gl.uniform_2f(self.u_tex_scale, tex_w, tex_h);
            self.gl
                .uniform_2f(self.u_frame_size, frame_size.0 as GLfloat, frame_size.1 as GLfloat);
            self.gl.tex_parameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
            self.gl.tex_parameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
            self.gl
                .tex_parameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
            self.gl
                .tex_parameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);

            let out_w = output_size.0 as GLfloat;
            let out_h = output_size.1 as GLfloat;
            let x0 = dst.x() as GLfloat / out_w * 2.0 - 1.0;
            let x1 = (dst.x() as GLfloat + dst.width() as GLfloat) / out_w * 2.0 - 1.0;
            let y0 = 1.0 - dst.y() as GLfloat / out_h * 2.0;
            let y1 = 1.0 - (dst.y() as GLfloat + dst.height() as GLfloat) / out_h * 2.0;

            self.gl.begin(GL_TRIANGLE_STRIP);
            self.gl.tex_coord_2f(0.0, 0.0);
            self.gl.vertex_2f(x0, y0);
            self.gl.tex_coord_2f(1.0, 0.0);
            self.gl.vertex_2f(x1, y0);
            self.gl.tex_coord_2f(0.0, 1.0);
            self.gl.vertex_2f(x0, y1);
            self.gl.tex_coord_2f(1.0, 1.0);
            self.gl.vertex_2f(x1, y1);
            self.gl.end();

            let unbind_result = sdl2::sys::SDL_GL_UnbindTexture(texture.raw());
            state.restore(&self.gl);
            if unbind_result != 0 {
                return Err(sdl2::get_error());
            }
            let _ = sdl2::sys::SDL_RenderFlush(canvas.raw());
        }

        Ok(())
    }
}

impl Drop for GlCrtRenderer {
    fn drop(&mut self) {
        unsafe {
            self.gl.delete_program(self.program);
        }
    }
}

struct GlFns {
    active_texture: unsafe extern "system" fn(GLenum),
    attach_shader: unsafe extern "system" fn(GLuint, GLuint),
    begin: unsafe extern "system" fn(GLenum),
    bind_texture: unsafe extern "system" fn(GLenum, GLuint),
    compile_shader: unsafe extern "system" fn(GLuint),
    create_program: unsafe extern "system" fn() -> GLuint,
    create_shader: unsafe extern "system" fn(GLenum) -> GLuint,
    delete_program: unsafe extern "system" fn(GLuint),
    delete_shader: unsafe extern "system" fn(GLuint),
    disable: unsafe extern "system" fn(GLenum),
    enable: unsafe extern "system" fn(GLenum),
    end: unsafe extern "system" fn(),
    get_program_info_log: unsafe extern "system" fn(GLuint, GLsizei, *mut GLsizei, *mut GLchar),
    get_program_iv: unsafe extern "system" fn(GLuint, GLenum, *mut GLint),
    get_shader_info_log: unsafe extern "system" fn(GLuint, GLsizei, *mut GLsizei, *mut GLchar),
    get_shader_iv: unsafe extern "system" fn(GLuint, GLenum, *mut GLint),
    get_integer_v: unsafe extern "system" fn(GLenum, *mut GLint),
    get_string: unsafe extern "system" fn(GLenum) -> *const u8,
    get_uniform_location: unsafe extern "system" fn(GLuint, *const GLchar) -> GLint,
    is_enabled: unsafe extern "system" fn(GLenum) -> u8,
    link_program: unsafe extern "system" fn(GLuint),
    shader_source: unsafe extern "system" fn(GLuint, GLsizei, *const *const GLchar, *const GLint),
    tex_coord_2f: unsafe extern "system" fn(GLfloat, GLfloat),
    tex_parameteri: unsafe extern "system" fn(GLenum, GLenum, GLint),
    uniform_1i: unsafe extern "system" fn(GLint, GLint),
    uniform_2f: unsafe extern "system" fn(GLint, GLfloat, GLfloat),
    use_program: unsafe extern "system" fn(GLuint),
    vertex_2f: unsafe extern "system" fn(GLfloat, GLfloat),
    viewport: unsafe extern "system" fn(GLint, GLint, GLsizei, GLsizei),
}

impl GlFns {
    fn load(video: &VideoSubsystem) -> Result<Self, String> {
        unsafe {
            Ok(Self {
                active_texture: load_gl(video, "glActiveTexture")?,
                attach_shader: load_gl(video, "glAttachShader")?,
                begin: load_gl(video, "glBegin")?,
                bind_texture: load_gl(video, "glBindTexture")?,
                compile_shader: load_gl(video, "glCompileShader")?,
                create_program: load_gl(video, "glCreateProgram")?,
                create_shader: load_gl(video, "glCreateShader")?,
                delete_program: load_gl(video, "glDeleteProgram")?,
                delete_shader: load_gl(video, "glDeleteShader")?,
                disable: load_gl(video, "glDisable")?,
                enable: load_gl(video, "glEnable")?,
                end: load_gl(video, "glEnd")?,
                get_program_info_log: load_gl(video, "glGetProgramInfoLog")?,
                get_program_iv: load_gl(video, "glGetProgramiv")?,
                get_shader_info_log: load_gl(video, "glGetShaderInfoLog")?,
                get_shader_iv: load_gl(video, "glGetShaderiv")?,
                get_integer_v: load_gl(video, "glGetIntegerv")?,
                get_string: load_gl(video, "glGetString")?,
                get_uniform_location: load_gl(video, "glGetUniformLocation")?,
                is_enabled: load_gl(video, "glIsEnabled")?,
                link_program: load_gl(video, "glLinkProgram")?,
                shader_source: load_gl(video, "glShaderSource")?,
                tex_coord_2f: load_gl(video, "glTexCoord2f")?,
                tex_parameteri: load_gl(video, "glTexParameteri")?,
                uniform_1i: load_gl(video, "glUniform1i")?,
                uniform_2f: load_gl(video, "glUniform2f")?,
                use_program: load_gl(video, "glUseProgram")?,
                vertex_2f: load_gl(video, "glVertex2f")?,
                viewport: load_gl(video, "glViewport")?,
            })
        }
    }

    unsafe fn active_texture(&self, texture: GLenum) {
        (self.active_texture)(texture);
    }

    unsafe fn attach_shader(&self, program: GLuint, shader: GLuint) {
        (self.attach_shader)(program, shader);
    }

    unsafe fn begin(&self, mode: GLenum) {
        (self.begin)(mode);
    }

    unsafe fn bind_texture(&self, target: GLenum, texture: GLuint) {
        (self.bind_texture)(target, texture);
    }

    unsafe fn compile_shader(&self, shader: GLuint) {
        (self.compile_shader)(shader);
    }

    unsafe fn create_program(&self) -> GLuint {
        (self.create_program)()
    }

    unsafe fn create_shader(&self, kind: GLenum) -> GLuint {
        (self.create_shader)(kind)
    }

    unsafe fn delete_program(&self, program: GLuint) {
        (self.delete_program)(program);
    }

    unsafe fn delete_shader(&self, shader: GLuint) {
        (self.delete_shader)(shader);
    }

    unsafe fn disable(&self, cap: GLenum) {
        (self.disable)(cap);
    }

    unsafe fn enable(&self, cap: GLenum) {
        (self.enable)(cap);
    }

    unsafe fn end(&self) {
        (self.end)();
    }

    unsafe fn get_program_info_log(
        &self,
        program: GLuint,
        max_len: GLsizei,
        len: *mut GLsizei,
        log: *mut GLchar,
    ) {
        (self.get_program_info_log)(program, max_len, len, log);
    }

    unsafe fn get_program_iv(&self, program: GLuint, pname: GLenum, value: *mut GLint) {
        (self.get_program_iv)(program, pname, value);
    }

    unsafe fn get_shader_info_log(
        &self,
        shader: GLuint,
        max_len: GLsizei,
        len: *mut GLsizei,
        log: *mut GLchar,
    ) {
        (self.get_shader_info_log)(shader, max_len, len, log);
    }

    unsafe fn get_shader_iv(&self, shader: GLuint, pname: GLenum, value: *mut GLint) {
        (self.get_shader_iv)(shader, pname, value);
    }

    unsafe fn get_integer_v(&self, name: GLenum, value: *mut GLint) {
        (self.get_integer_v)(name, value);
    }

    unsafe fn get_string(&self, name: GLenum) -> *const u8 {
        (self.get_string)(name)
    }

    unsafe fn get_uniform_location(&self, program: GLuint, name: *const GLchar) -> GLint {
        (self.get_uniform_location)(program, name)
    }

    unsafe fn is_enabled(&self, cap: GLenum) -> bool {
        (self.is_enabled)(cap) != 0
    }

    unsafe fn link_program(&self, program: GLuint) {
        (self.link_program)(program);
    }

    unsafe fn shader_source(
        &self,
        shader: GLuint,
        count: GLsizei,
        strings: *const *const GLchar,
        lengths: *const GLint,
    ) {
        (self.shader_source)(shader, count, strings, lengths);
    }

    unsafe fn tex_coord_2f(&self, s: GLfloat, t: GLfloat) {
        (self.tex_coord_2f)(s, t);
    }

    unsafe fn tex_parameteri(&self, target: GLenum, pname: GLenum, value: GLint) {
        (self.tex_parameteri)(target, pname, value);
    }

    unsafe fn uniform_1i(&self, location: GLint, value: GLint) {
        (self.uniform_1i)(location, value);
    }

    unsafe fn uniform_2f(&self, location: GLint, x: GLfloat, y: GLfloat) {
        (self.uniform_2f)(location, x, y);
    }

    unsafe fn use_program(&self, program: GLuint) {
        (self.use_program)(program);
    }

    unsafe fn vertex_2f(&self, x: GLfloat, y: GLfloat) {
        (self.vertex_2f)(x, y);
    }

    unsafe fn viewport(&self, x: GLint, y: GLint, w: GLsizei, h: GLsizei) {
        (self.viewport)(x, y, w, h);
    }
}

struct GlStateSnapshot {
    active_texture: GLint,
    bound_texture_2d: GLint,
    current_program: GLint,
    viewport: [GLint; 4],
    blend_enabled: bool,
    cull_enabled: bool,
    depth_enabled: bool,
    texture_2d_enabled: bool,
}

impl GlStateSnapshot {
    unsafe fn capture(gl: &GlFns) -> Self {
        let mut active_texture = 0;
        let mut bound_texture_2d = 0;
        let mut current_program = 0;
        let mut viewport = [0; 4];
        gl.get_integer_v(GL_ACTIVE_TEXTURE, &mut active_texture);
        gl.active_texture(GL_TEXTURE0);
        gl.get_integer_v(GL_TEXTURE_BINDING_2D, &mut bound_texture_2d);
        gl.get_integer_v(GL_CURRENT_PROGRAM, &mut current_program);
        gl.get_integer_v(GL_VIEWPORT, viewport.as_mut_ptr());
        let texture_2d_enabled = gl.is_enabled(GL_TEXTURE_2D);
        gl.active_texture(active_texture as GLenum);
        Self {
            active_texture,
            bound_texture_2d,
            current_program,
            viewport,
            blend_enabled: gl.is_enabled(GL_BLEND),
            cull_enabled: gl.is_enabled(GL_CULL_FACE),
            depth_enabled: gl.is_enabled(GL_DEPTH_TEST),
            texture_2d_enabled,
        }
    }

    unsafe fn restore(&self, gl: &GlFns) {
        gl.active_texture(GL_TEXTURE0);
        set_enabled(gl, GL_TEXTURE_2D, self.texture_2d_enabled);
        gl.bind_texture(GL_TEXTURE_2D, self.bound_texture_2d as GLuint);
        gl.active_texture(self.active_texture as GLenum);
        set_enabled(gl, GL_BLEND, self.blend_enabled);
        set_enabled(gl, GL_CULL_FACE, self.cull_enabled);
        set_enabled(gl, GL_DEPTH_TEST, self.depth_enabled);
        gl.use_program(self.current_program as GLuint);
        gl.viewport(
            self.viewport[0],
            self.viewport[1],
            self.viewport[2],
            self.viewport[3],
        );
    }
}

unsafe fn set_enabled(gl: &GlFns, cap: GLenum, enabled: bool) {
    if enabled {
        gl.enable(cap);
    } else {
        gl.disable(cap);
    }
}

unsafe fn load_gl<T>(video: &VideoSubsystem, name: &str) -> Result<T, String> {
    let ptr = video.gl_get_proc_address(name);
    if ptr.is_null() {
        return Err(format!("missing OpenGL function {name}"));
    }
    Ok(std::mem::transmute_copy(&ptr))
}

unsafe fn compile_program(gl: &GlFns, vertex_src: &str, fragment_src: &str) -> Result<GLuint, String> {
    let vertex = compile_shader(gl, GL_VERTEX_SHADER, vertex_src)?;
    let fragment = compile_shader(gl, GL_FRAGMENT_SHADER, fragment_src)?;
    let program = gl.create_program();
    if program == 0 {
        gl.delete_shader(vertex);
        gl.delete_shader(fragment);
        return Err("glCreateProgram returned 0".to_string());
    }
    gl.attach_shader(program, vertex);
    gl.attach_shader(program, fragment);
    gl.link_program(program);
    gl.delete_shader(vertex);
    gl.delete_shader(fragment);

    let mut linked = 0;
    gl.get_program_iv(program, GL_LINK_STATUS, &mut linked);
    if linked == 0 {
        let log = program_log(gl, program);
        gl.delete_program(program);
        Err(format!("CRT shader link failed: {log}"))
    } else {
        Ok(program)
    }
}

unsafe fn compile_shader(gl: &GlFns, kind: GLenum, source: &str) -> Result<GLuint, String> {
    let shader = gl.create_shader(kind);
    if shader == 0 {
        return Err("glCreateShader returned 0".to_string());
    }
    let source = CString::new(source).map_err(|e| e.to_string())?;
    let ptr = source.as_ptr();
    gl.shader_source(shader, 1, &ptr, std::ptr::null());
    gl.compile_shader(shader);

    let mut compiled = 0;
    gl.get_shader_iv(shader, GL_COMPILE_STATUS, &mut compiled);
    if compiled == 0 {
        let log = shader_log(gl, shader);
        gl.delete_shader(shader);
        Err(format!("CRT shader compile failed: {log}"))
    } else {
        Ok(shader)
    }
}

fn uniform_location(gl: &GlFns, program: GLuint, name: &str) -> Result<GLint, String> {
    let name = CString::new(name).map_err(|e| e.to_string())?;
    let location = unsafe { gl.get_uniform_location(program, name.as_ptr()) };
    if location < 0 {
        Err(format!("CRT shader missing uniform {}", name.to_string_lossy()))
    } else {
        Ok(location)
    }
}

unsafe fn shader_log(gl: &GlFns, shader: GLuint) -> String {
    let mut len = 0;
    gl.get_shader_iv(shader, GL_INFO_LOG_LENGTH, &mut len);
    if len <= 1 {
        return "no shader compiler log".to_string();
    }
    let mut buf = vec![0_i8; len as usize];
    let mut written = 0;
    gl.get_shader_info_log(shader, len, &mut written, buf.as_mut_ptr());
    CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned()
}

unsafe fn program_log(gl: &GlFns, program: GLuint) -> String {
    let mut len = 0;
    gl.get_program_iv(program, GL_INFO_LOG_LENGTH, &mut len);
    if len <= 1 {
        return "no program linker log".to_string();
    }
    let mut buf = vec![0_i8; len as usize];
    let mut written = 0;
    gl.get_program_info_log(program, len, &mut written, buf.as_mut_ptr());
    CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned()
}

const CRT_VERTEX_SHADER: &str = r#"
#version 120
varying vec2 v_uv;

void main() {
    gl_Position = gl_Vertex;
    v_uv = gl_MultiTexCoord0.xy;
}
"#;

const CRT_FRAGMENT_SHADER: &str = r#"
#version 120
uniform sampler2D u_frame;
uniform vec2 u_tex_scale;
uniform vec2 u_frame_size;
uniform int u_mode;
varying vec2 v_uv;

vec3 fetch_frame(vec2 uv) {
    if (uv.x < 0.0 || uv.y < 0.0 || uv.x > 1.0 || uv.y > 1.0) {
        return vec3(0.0);
    }
    return texture2D(u_frame, uv * u_tex_scale).rgb;
}

vec2 crt_warp(vec2 uv) {
    float warp = 0.045;
    float warp2 = 0.014;
    if (u_mode == 1) {
        warp = 0.078;
        warp2 = 0.024;
    } else if (u_mode == 2) {
        warp = 0.018;
        warp2 = 0.004;
    }
    vec2 centered = uv * 2.0 - 1.0;
    float r2 = dot(centered, centered);
    centered *= 1.0 + warp * r2 + warp2 * r2 * r2;
    return centered * 0.5 + 0.5;
}

vec3 saturate_crt(vec3 c) {
    float luma = dot(c, vec3(0.299, 0.587, 0.114));
    float sat = 1.22;
    vec3 warmth = vec3(1.08, 1.02, 0.92);
    if (u_mode == 1) {
        sat = 1.34;
        warmth = vec3(1.16, 1.04, 0.84);
    } else if (u_mode == 2) {
        sat = 1.12;
        warmth = vec3(0.98, 1.03, 1.08);
    }
    c = mix(vec3(luma), c, sat);
    c *= warmth;
    return max(c, vec3(0.0));
}

void main() {
    vec2 uv = crt_warp(v_uv);
    vec2 texel = 1.0 / u_frame_size;

    vec3 core = fetch_frame(uv);
    vec3 soft = vec3(0.0);
    soft += fetch_frame(uv + vec2(texel.x * 1.25, 0.0));
    soft += fetch_frame(uv - vec2(texel.x * 1.25, 0.0));
    soft += fetch_frame(uv + vec2(0.0, texel.y * 1.25));
    soft += fetch_frame(uv - vec2(0.0, texel.y * 1.25));
    soft += fetch_frame(uv + texel * vec2(1.15, 1.15));
    soft += fetch_frame(uv + texel * vec2(-1.15, 1.15));
    soft += fetch_frame(uv + texel * vec2(1.15, -1.15));
    soft += fetch_frame(uv + texel * vec2(-1.15, -1.15));
    soft *= 0.125;

    float hot = smoothstep(0.42, 0.92, dot(core, vec3(0.299, 0.587, 0.114)));
    float core_gain = 0.94;
    float bloom_base = 0.15;
    float bloom_hot = 0.20;
    if (u_mode == 1) {
        core_gain = 0.88;
        bloom_base = 0.24;
        bloom_hot = 0.32;
    } else if (u_mode == 2) {
        core_gain = 1.02;
        bloom_base = 0.055;
        bloom_hot = 0.08;
    }
    vec3 color = core * core_gain + soft * (bloom_base + hot * bloom_hot);
    color = saturate_crt(color);

    float source_line = uv.y * u_frame_size.y;
    float beam = abs(sin(source_line * 3.14159265));
    float scan_floor = 0.56;
    float scan_peak = 1.12;
    float scan_power = 0.68;
    if (u_mode == 1) {
        scan_floor = 0.46;
        scan_peak = 1.18;
        scan_power = 0.58;
    } else if (u_mode == 2) {
        scan_floor = 0.74;
        scan_peak = 1.06;
        scan_power = 0.82;
    }
    float scan = mix(scan_floor, scan_peak, pow(beam, scan_power));
    color *= scan;

    float triad = mod(gl_FragCoord.x, 3.0);
    vec3 mask = vec3(0.72);
    if (triad < 1.0) {
        mask = vec3(1.14, 0.72, 0.70);
    } else if (triad < 2.0) {
        mask = vec3(0.74, 1.10, 0.72);
    } else {
        mask = vec3(0.72, 0.78, 1.18);
    }
    float slot = mod(gl_FragCoord.y, 6.0) < 3.0 ? 1.0 : 0.86;
    float mask_strength = 0.34;
    if (u_mode == 1) {
        slot = mod(gl_FragCoord.y, 8.0) < 4.0 ? 1.0 : 0.76;
        mask_strength = 0.46;
    } else if (u_mode == 2) {
        slot = 0.95;
        mask_strength = 0.23;
    }
    color *= mix(vec3(1.0), mask * slot, mask_strength);

    vec2 edge = abs(uv * 2.0 - 1.0);
    float vignette = 1.0 - smoothstep(0.62, 1.35, dot(edge, edge));
    float vignette_floor = 0.42;
    if (u_mode == 1) {
        vignette_floor = 0.28;
    } else if (u_mode == 2) {
        vignette_floor = 0.70;
    }
    color *= mix(vignette_floor, 1.0, vignette);

    float edge_fade = smoothstep(0.0, 0.02, uv.x) *
        smoothstep(0.0, 0.02, uv.y) *
        smoothstep(0.0, 0.02, 1.0 - uv.x) *
        smoothstep(0.0, 0.02, 1.0 - uv.y);
    color *= edge_fade;

    color = pow(max(color, vec3(0.0)), vec3(0.92));
    gl_FragColor = vec4(color, 1.0);
}
"#;
