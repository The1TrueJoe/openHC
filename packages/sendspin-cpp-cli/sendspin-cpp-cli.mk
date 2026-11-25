################################################################################
#
# sendspin-cpp-cli -- headless Sendspin player (sendspin-cli)
#
# Not in Buildroot, so openHC carries it. audiod runs one per Sendspin endpoint.
#
# Upstream's CMake fetches sendspin-cpp and its libraries with FetchContent at
# configure time. Buildroot downloads them instead (EXTRA_DOWNLOADS), they are
# unpacked under deps/, and CMake is pointed at them FULLYDISCONNECTED, as the
# README's "Building from source" describes. Each is pinned to the tag (or the
# submodule commit) that sendspin-cli v1.0.1 / sendspin-cpp v0.10.0 ask for:
# sendspin-cli's CMakeLists.txt (sendspin-cpp) and sendspin-cpp's
# cmake/host.cmake (the rest). FetchContent's GIT_SUBMODULES are unpacked into
# their parent's lib/. The TUI (FTXUI) and the tests (GoogleTest) are off, so
# neither is needed.
#
# No .hash file, as for librespot: GitHub's /archive/ tarballs are not
# byte-stable, so a pinned sha256 fails at random.
#
################################################################################

SENDSPIN_CPP_CLI_VERSION = v1.0.1
SENDSPIN_CPP_CLI_SITE = $(call github,Sendspin,sendspin-cpp-cli,$(SENDSPIN_CPP_CLI_VERSION))
SENDSPIN_CPP_CLI_LICENSE = Apache-2.0
SENDSPIN_CPP_CLI_LICENSE_FILES = LICENSE
SENDSPIN_CPP_CLI_DEPENDENCIES = alsa-lib

# <dir under deps/>:<GitHub owner/repo>:<tag or commit>, a parent before its
# submodules.
SENDSPIN_CPP_CLI_SOURCES = \
	sendspin-cpp:Sendspin/sendspin-cpp:v0.10.0 \
	ArduinoJson:bblanchon/ArduinoJson:v7.4.1 \
	micro-flac:esphome-libs/micro-flac:v0.3.1 \
	micro-flac/lib/micro-ogg-demuxer:esphome-libs/micro-ogg-demuxer:ee95ec6f6b633789ec31e0e44877f6cc7fe7cf67 \
	micro-opus:esphome-libs/micro-opus:v0.3.5 \
	micro-opus/lib/micro-ogg-demuxer:esphome-libs/micro-ogg-demuxer:865ad9d831e7dc76bb9c142607bae33fc75648e7 \
	micro-opus/lib/opus:xiph/opus:22244de5a79bd1d6d623c32e72bf1954b56235be \
	IXWebSocket:machinezone/IXWebSocket:v12.0.1 \
	noise-c:esphome-libs/noise-c:v0.1.13

sendspin-field = $(word $(2),$(subst :, ,$(1)))
# <repo>-<ref>.tar.gz: unique per source, so the downloads cannot collide.
sendspin-tarball = $(notdir $(call sendspin-field,$(1),2))-$(call sendspin-field,$(1),3).tar.gz

SENDSPIN_CPP_CLI_EXTRA_DOWNLOADS = $(foreach s,$(SENDSPIN_CPP_CLI_SOURCES),\
	https://github.com/$(call sendspin-field,$(s),2)/archive/$(call sendspin-field,$(s),3)/$(call sendspin-tarball,$(s)))

# One source into deps/<dir> (a shell command ending in &&).
sendspin-extract = mkdir -p $(@D)/deps/$(call sendspin-field,$(1),1) && $(call suitable-extractor,$(call sendspin-tarball,$(1))) $(SENDSPIN_CPP_CLI_DL_DIR)/$(call sendspin-tarball,$(1)) | $(TAR) --strip-components=1 -C $(@D)/deps/$(call sendspin-field,$(1),1) $(TAR_OPTIONS) - &&

define SENDSPIN_CPP_CLI_EXTRACT_DEPS
	$(foreach s,$(SENDSPIN_CPP_CLI_SOURCES),$(call sendspin-extract,$(s))) true
endef
SENDSPIN_CPP_CLI_POST_EXTRACT_HOOKS += SENDSPIN_CPP_CLI_EXTRACT_DEPS

SENDSPIN_CPP_CLI_DEPS_DIR = $(SENDSPIN_CPP_CLI_DIR)/deps
SENDSPIN_CPP_CLI_CONF_OPTS = \
	-DFETCHCONTENT_FULLYDISCONNECTED=ON \
	-DFETCHCONTENT_SOURCE_DIR_SENDSPIN=$(SENDSPIN_CPP_CLI_DEPS_DIR)/sendspin-cpp \
	-DFETCHCONTENT_SOURCE_DIR_ARDUINOJSON=$(SENDSPIN_CPP_CLI_DEPS_DIR)/ArduinoJson \
	-DFETCHCONTENT_SOURCE_DIR_MICRO_FLAC=$(SENDSPIN_CPP_CLI_DEPS_DIR)/micro-flac \
	-DFETCHCONTENT_SOURCE_DIR_MICRO_OPUS=$(SENDSPIN_CPP_CLI_DEPS_DIR)/micro-opus \
	-DFETCHCONTENT_SOURCE_DIR_IXWEBSOCKET=$(SENDSPIN_CPP_CLI_DEPS_DIR)/IXWebSocket \
	-DFETCHCONTENT_SOURCE_DIR_NOISE_C=$(SENDSPIN_CPP_CLI_DEPS_DIR)/noise-c \
	-DSENDSPIN_CLI_BUILD_TESTS=OFF \
	-DSENDSPIN_CLI_WITH_TUI=OFF \
	-DSENDSPIN_CLI_WITH_ALSA=ON \
	-DSENDSPIN_CLI_WITH_PORTAUDIO=OFF \
	-DSENDSPIN_CLI_WITH_PULSE=OFF \
	-DSENDSPIN_CLI_WITH_PIPEWIRE=OFF

# mDNS (how a server finds the player) is Avahi's libdns_sd compatibility
# library; upstream finds dns_sd.h and libdns_sd on its own.
ifeq ($(BR2_PACKAGE_AVAHI_LIBDNSSD_COMPATIBILITY),y)
SENDSPIN_CPP_CLI_DEPENDENCIES += avahi
SENDSPIN_CPP_CLI_CONF_OPTS += -DSENDSPIN_CLI_WITH_MDNS=ON
else
SENDSPIN_CPP_CLI_CONF_OPTS += -DSENDSPIN_CLI_WITH_MDNS=OFF
endif

# Only the binary: upstream's install also drops docs, systemd units and
# ArduinoJson's headers. audiod supervises it, so there is no init script.
define SENDSPIN_CPP_CLI_INSTALL_TARGET_CMDS
	$(INSTALL) -D -m 0755 $(SENDSPIN_CPP_CLI_BUILDDIR)/sendspin-cli \
		$(TARGET_DIR)/usr/bin/sendspin-cli
endef

$(eval $(cmake-package))
