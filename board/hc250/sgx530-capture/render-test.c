// SPDX-License-Identifier: GPL-2.0
// SGX530 render-capture test: a 64x64 clear plus one flat-colour triangle through
// whatever EGL/GLES2 libraries are on the library path, then a pixel readback.
//
// The GL entry points are looked up with dlopen/dlsym, so this builds with no
// vendor headers or libraries (constants are the Khronos enum values). It tries a
// 64x64 pbuffer first and falls back to a window surface. The readback goes to
// stdout (corners, centre, a checksum) and to /tmp/rt.rgba.
#include <dlfcn.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

typedef void *EGLDisplay, *EGLConfig, *EGLSurface, *EGLContext;
typedef int32_t EGLint;
typedef unsigned int GLuint, GLenum, GLbitfield;
typedef int GLint, GLsizei;
typedef float GLfloat;

#define EGL_SURFACE_TYPE 0x3033
#define EGL_PBUFFER_BIT 0x0001
#define EGL_WINDOW_BIT 0x0004
#define EGL_RENDERABLE_TYPE 0x3040
#define EGL_OPENGL_ES2_BIT 0x0004
#define EGL_RED_SIZE 0x3024
#define EGL_GREEN_SIZE 0x3023
#define EGL_BLUE_SIZE 0x3022
#define EGL_NONE 0x3038
#define EGL_WIDTH 0x3057
#define EGL_HEIGHT 0x3056
#define EGL_OPENGL_ES_API 0x30A0
#define EGL_CONTEXT_CLIENT_VERSION 0x3098

#define GL_VERTEX_SHADER 0x8B31
#define GL_FRAGMENT_SHADER 0x8B30
#define GL_COMPILE_STATUS 0x8B81
#define GL_LINK_STATUS 0x8B82
#define GL_COLOR_BUFFER_BIT 0x4000
#define GL_FLOAT 0x1406
#define GL_TRIANGLES 0x0004
#define GL_RGBA 0x1908
#define GL_UNSIGNED_BYTE 0x1401

#define W 64
#define H 64

static void *egl, *gles;

static void *sym(void *lib, const char *n)
{
	void *f = dlsym(lib, n);

	if (!f) {
		fprintf(stderr, "RT: missing %s\n", n);
		exit(3);
	}
	return f;
}

#define E(ret, name, args) ret(*name) args = (ret(*) args)sym(egl, #name)
#define G(ret, name, args) ret(*name) args = (ret(*) args)sym(gles, #name)

static const char *VS =
	"attribute vec4 aPos;\n"
	"void main(){ gl_Position = aPos; }\n";
static const char *FS =
	"precision mediump float;\n"
	"uniform vec4 uColor;\n"
	"void main(){ gl_FragColor = uColor; }\n";

int main(void)
{
	egl = dlopen("libEGL.so.1", RTLD_NOW | RTLD_GLOBAL);
	gles = dlopen("libGLESv2.so.2", RTLD_NOW | RTLD_GLOBAL);
	if (!egl || !gles) {
		fprintf(stderr, "RT: dlopen: %s\n", dlerror());
		return 2;
	}
	E(EGLDisplay, eglGetDisplay, (void *));
	E(int, eglInitialize, (EGLDisplay, EGLint *, EGLint *));
	E(int, eglChooseConfig, (EGLDisplay, const EGLint *, EGLConfig *, EGLint, EGLint *));
	E(EGLSurface, eglCreatePbufferSurface, (EGLDisplay, EGLConfig, const EGLint *));
	E(EGLSurface, eglCreateWindowSurface, (EGLDisplay, EGLConfig, void *, const EGLint *));
	E(int, eglBindAPI, (EGLint));
	E(EGLContext, eglCreateContext, (EGLDisplay, EGLConfig, EGLContext, const EGLint *));
	E(int, eglMakeCurrent, (EGLDisplay, EGLSurface, EGLSurface, EGLContext));
	E(EGLint, eglGetError, (void));
	G(GLuint, glCreateShader, (GLenum));
	G(void, glShaderSource, (GLuint, GLsizei, const char *const *, const GLint *));
	G(void, glCompileShader, (GLuint));
	G(void, glGetShaderiv, (GLuint, GLenum, GLint *));
	G(GLuint, glCreateProgram, (void));
	G(void, glAttachShader, (GLuint, GLuint));
	G(void, glBindAttribLocation, (GLuint, GLuint, const char *));
	G(void, glLinkProgram, (GLuint));
	G(void, glGetProgramiv, (GLuint, GLenum, GLint *));
	G(void, glUseProgram, (GLuint));
	G(GLint, glGetUniformLocation, (GLuint, const char *));
	G(void, glUniform4f, (GLint, GLfloat, GLfloat, GLfloat, GLfloat));
	G(void, glViewport, (GLint, GLint, GLsizei, GLsizei));
	G(void, glClearColor, (GLfloat, GLfloat, GLfloat, GLfloat));
	G(void, glClear, (GLbitfield));
	G(void, glVertexAttribPointer, (GLuint, GLint, GLenum, int, GLsizei, const void *));
	G(void, glEnableVertexAttribArray, (GLuint));
	G(void, glDrawArrays, (GLenum, GLint, GLsizei));
	G(void, glReadPixels, (GLint, GLint, GLsizei, GLsizei, GLenum, GLenum, void *));
	G(void, glFinish, (void));
	G(GLenum, glGetError, (void));

	EGLDisplay d = eglGetDisplay(0);
	EGLint maj = 0, min = 0, n = 0;
	if (!d || !eglInitialize(d, &maj, &min)) {
		fprintf(stderr, "RT: eglInitialize failed 0x%x\n", eglGetError());
		return 2;
	}
	printf("RT: EGL %d.%d\n", maj, min);

	EGLint pba[] = { EGL_SURFACE_TYPE, EGL_PBUFFER_BIT, EGL_RENDERABLE_TYPE, EGL_OPENGL_ES2_BIT,
			 EGL_RED_SIZE, 8, EGL_GREEN_SIZE, 8, EGL_BLUE_SIZE, 8, EGL_NONE };
	EGLint wna[] = { EGL_SURFACE_TYPE, EGL_WINDOW_BIT, EGL_RENDERABLE_TYPE, EGL_OPENGL_ES2_BIT,
			 EGL_RED_SIZE, 8, EGL_GREEN_SIZE, 8, EGL_BLUE_SIZE, 8, EGL_NONE };
	EGLint pbs[] = { EGL_WIDTH, W, EGL_HEIGHT, H, EGL_NONE };
	EGLConfig c = 0;
	EGLSurface s = 0;
	if (eglChooseConfig(d, pba, &c, 1, &n) && n > 0)
		s = eglCreatePbufferSurface(d, c, pbs);
	if (s) {
		printf("RT: pbuffer surface\n");
	} else {
		fprintf(stderr, "RT: no pbuffer (0x%x), trying a window\n", eglGetError());
		if (!eglChooseConfig(d, wna, &c, 1, &n) || n < 1 ||
		    !(s = eglCreateWindowSurface(d, c, 0, 0))) {
			fprintf(stderr, "RT: no window surface 0x%x\n", eglGetError());
			return 2;
		}
		printf("RT: window surface\n");
	}
	eglBindAPI(EGL_OPENGL_ES_API);
	EGLint ca[] = { EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE };
	EGLContext ctx = eglCreateContext(d, c, 0, ca);
	if (!ctx || !eglMakeCurrent(d, s, s, ctx)) {
		fprintf(stderr, "RT: context/makecurrent failed 0x%x\n", eglGetError());
		return 2;
	}

	GLuint vs = glCreateShader(GL_VERTEX_SHADER), fs = glCreateShader(GL_FRAGMENT_SHADER), p;
	GLint ok = 0;
	glShaderSource(vs, 1, &VS, 0);
	glCompileShader(vs);
	glGetShaderiv(vs, GL_COMPILE_STATUS, &ok);
	if (!ok)
		fprintf(stderr, "RT: vs compile failed\n");
	glShaderSource(fs, 1, &FS, 0);
	glCompileShader(fs);
	glGetShaderiv(fs, GL_COMPILE_STATUS, &ok);
	if (!ok)
		fprintf(stderr, "RT: fs compile failed\n");
	p = glCreateProgram();
	glAttachShader(p, vs);
	glAttachShader(p, fs);
	glBindAttribLocation(p, 0, "aPos");
	glLinkProgram(p);
	glGetProgramiv(p, GL_LINK_STATUS, &ok);
	if (!ok)
		fprintf(stderr, "RT: link failed\n");
	glUseProgram(p);

	/* clear to (0, 0, 0.5, 1), then a red triangle over the lower-left half */
	static const GLfloat tri[] = { -1, -1, 0, 1, 1, -1, 0, 1, -1, 1, 0, 1 };
	glViewport(0, 0, W, H);
	glClearColor(0.0f, 0.0f, 0.5f, 1.0f);
	glClear(GL_COLOR_BUFFER_BIT);
	glUniform4f(glGetUniformLocation(p, "uColor"), 1.0f, 0.0f, 0.0f, 1.0f);
	glVertexAttribPointer(0, 4, GL_FLOAT, 0, 0, tri);
	glEnableVertexAttribArray(0);
	glDrawArrays(GL_TRIANGLES, 0, 3);
	glFinish();

	static uint8_t px[W * H * 4];
	uint32_t sum = 0;
	glReadPixels(0, 0, W, H, GL_RGBA, GL_UNSIGNED_BYTE, px);
	printf("RT: glGetError 0x%x\n", glGetError());
	for (int i = 0; i < W * H * 4; i++)
		sum = sum * 31 + px[i];
#define PX(x, y) px[((y) * W + (x)) * 4 + 0], px[((y) * W + (x)) * 4 + 1], px[((y) * W + (x)) * 4 + 2], px[((y) * W + (x)) * 4 + 3]
	printf("RT: (2,2)=%02x%02x%02x%02x (61,61)=%02x%02x%02x%02x (61,2)=%02x%02x%02x%02x (2,61)=%02x%02x%02x%02x sum=%08x\n",
	       PX(2, 2), PX(61, 61), PX(61, 2), PX(2, 61), sum);
	FILE *f = fopen("/tmp/rt.rgba", "wb");
	if (f) {
		fwrite(px, 1, sizeof(px), f);
		fclose(f);
	}
	return 0;
}
