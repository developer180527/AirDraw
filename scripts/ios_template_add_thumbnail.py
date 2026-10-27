#!/usr/bin/env python3
"""Adds AirDraw's Quick Look thumbnail extension to a copy of Godot's iOS Xcode template.

Usage: ios_template_add_thumbnail.py <template dir> <extension source dir>

Copies the Swift source and Info.plist into <template>/AirDrawThumbnail/ and inserts an
app-extension target into project.pbxproj, embedded in the main app and built before it.
Godot's exporter replaces only the first $placeholder per line, so every line added here
holds at most one.
"""
import shutil
import sys
from pathlib import Path

template, source = Path(sys.argv[1]), Path(sys.argv[2])
ext_dir = template / "AirDrawThumbnail"
ext_dir.mkdir(exist_ok=True)
shutil.copy(source / "ThumbnailProvider.swift", ext_dir / "ThumbnailProvider.swift")
shutil.copy(source / "Info-iOS.plist", ext_dir / "Info.plist")

pbx = template / "godot_apple_embedded.xcodeproj" / "project.pbxproj"
s = pbx.read_text()
if "AirDrawThumbnail" in s:
    sys.exit(0)  # already added

ID = {k: f"AD000000000000000000{n:04X}" for n, k in enumerate(
    ["src_build", "src_ref", "plist_ref", "product", "group", "sources", "frameworks", "target",
     "debug", "release", "configs", "embed_file", "embed_phase", "proxy", "dependency"], start=1)}
MAIN_TARGET = "D0BCFE3318AEBDA2004A7AAE"
PROJECT = "D0BCFE2C18AEBDA2004A7AAE"


def insert_before(marker: str, text: str) -> None:
    global s
    assert marker in s, marker
    s = s.replace(marker, text + marker, 1)


def insert_after(marker: str, text: str) -> None:
    global s
    assert marker in s, marker
    s = s.replace(marker, marker + text, 1)


def config(name: str, kind: str) -> str:
    return f"""		{ID[name]} /* {name.capitalize()} */ = {{
			isa = XCBuildConfiguration;
			buildSettings = {{
				APPLICATION_EXTENSION_API_ONLY = YES;
				CODE_SIGN_IDENTITY = "$code_sign_identity_{kind}";
				CODE_SIGN_STYLE = "$code_sign_style_{kind}";
				CURRENT_PROJECT_VERSION = $version;
				DEVELOPMENT_TEAM = $team_id;
				INFOPLIST_FILE = "AirDrawThumbnail/Info.plist";
				$os_deployment_target
				LD_RUNPATH_SEARCH_PATHS = (
					"$(inherited)",
					"@executable_path/Frameworks",
					"@executable_path/../../Frameworks",
				);
				MARKETING_VERSION = $short_version;
				OTHER_LDFLAGS = "";
				PRODUCT_BUNDLE_IDENTIFIER = "$bundle_identifier.thumbnail";
				PRODUCT_NAME = AirDrawThumbnail;
				SKIP_INSTALL = YES;
				SWIFT_VERSION = 5.0;
				TARGETED_DEVICE_FAMILY = "$targeted_device_family";
			}};
			name = {name.capitalize()};
		}};
"""


insert_before("/* End PBXBuildFile section */", f"""		{ID['src_build']} /* ThumbnailProvider.swift in Sources */ = {{isa = PBXBuildFile; fileRef = {ID['src_ref']} /* ThumbnailProvider.swift */; }};
		{ID['embed_file']} /* AirDrawThumbnail.appex in Embed App Extensions */ = {{isa = PBXBuildFile; fileRef = {ID['product']} /* AirDrawThumbnail.appex */; settings = {{ATTRIBUTES = (RemoveHeadersOnCopy, ); }}; }};
""")
insert_before("/* End PBXCopyFilesBuildPhase section */", f"""		{ID['embed_phase']} /* Embed App Extensions */ = {{
			isa = PBXCopyFilesBuildPhase;
			buildActionMask = 2147483647;
			dstPath = "";
			dstSubfolderSpec = 13;
			files = (
				{ID['embed_file']} /* AirDrawThumbnail.appex in Embed App Extensions */,
			);
			name = "Embed App Extensions";
			runOnlyForDeploymentPostprocessing = 0;
		}};
""")
insert_before("/* Begin PBXCopyFilesBuildPhase section */", f"""/* Begin PBXContainerItemProxy section */
		{ID['proxy']} /* PBXContainerItemProxy */ = {{
			isa = PBXContainerItemProxy;
			containerPortal = {PROJECT} /* Project object */;
			proxyType = 1;
			remoteGlobalIDString = {ID['target']};
			remoteInfo = AirDrawThumbnail;
		}};
/* End PBXContainerItemProxy section */

""")
insert_before("/* End PBXFileReference section */", f"""		{ID['src_ref']} /* ThumbnailProvider.swift */ = {{isa = PBXFileReference; lastKnownFileType = sourcecode.swift; path = ThumbnailProvider.swift; sourceTree = "<group>"; }};
		{ID['plist_ref']} /* Info.plist */ = {{isa = PBXFileReference; lastKnownFileType = text.plist.xml; path = Info.plist; sourceTree = "<group>"; }};
		{ID['product']} /* AirDrawThumbnail.appex */ = {{isa = PBXFileReference; explicitFileType = "wrapper.app-extension"; includeInIndex = 0; path = AirDrawThumbnail.appex; sourceTree = BUILT_PRODUCTS_DIR; }};
""")
insert_before("/* End PBXFrameworksBuildPhase section */", f"""		{ID['frameworks']} /* Frameworks */ = {{
			isa = PBXFrameworksBuildPhase;
			buildActionMask = 2147483647;
			files = (
			);
			runOnlyForDeploymentPostprocessing = 0;
		}};
""")
insert_after("				F965960C2BC2C3A800579C7E /* PrivacyInfo.xcprivacy */,\n",
             f"				{ID['group']} /* AirDrawThumbnail */,\n")
insert_after("				D0BCFE3418AEBDA2004A7AAE /* $binary.app */,\n",
             f"				{ID['product']} /* AirDrawThumbnail.appex */,\n")
insert_before("/* End PBXGroup section */", f"""		{ID['group']} /* AirDrawThumbnail */ = {{
			isa = PBXGroup;
			children = (
				{ID['src_ref']} /* ThumbnailProvider.swift */,
				{ID['plist_ref']} /* Info.plist */,
			);
			path = AirDrawThumbnail;
			sourceTree = "<group>";
		}};
""")
insert_after("				90A13CD024AA68E500E8464F /* Embed Frameworks */,\n",
             f"				{ID['embed_phase']} /* Embed App Extensions */,\n")
insert_after("			dependencies = (\n", f"				{ID['dependency']} /* PBXTargetDependency */,\n")
insert_before("/* End PBXNativeTarget section */", f"""		{ID['target']} /* AirDrawThumbnail */ = {{
			isa = PBXNativeTarget;
			buildConfigurationList = {ID['configs']} /* Build configuration list for PBXNativeTarget "AirDrawThumbnail" */;
			buildPhases = (
				{ID['sources']} /* Sources */,
				{ID['frameworks']} /* Frameworks */,
			);
			buildRules = (
			);
			dependencies = (
			);
			name = AirDrawThumbnail;
			productName = AirDrawThumbnail;
			productReference = {ID['product']} /* AirDrawThumbnail.appex */;
			productType = "com.apple.product-type.app-extension";
		}};
""")
insert_after("				TargetAttributes = {\n", f"""					{ID['target']} = {{
						DevelopmentTeam = $team_id;
						ProvisioningStyle = Automatic;
					}};
""")
insert_after(f"			targets = (\n				{MAIN_TARGET} /* $binary */,\n",
             f"				{ID['target']} /* AirDrawThumbnail */,\n")
insert_before("/* End PBXSourcesBuildPhase section */", f"""		{ID['sources']} /* Sources */ = {{
			isa = PBXSourcesBuildPhase;
			buildActionMask = 2147483647;
			files = (
				{ID['src_build']} /* ThumbnailProvider.swift in Sources */,
			);
			runOnlyForDeploymentPostprocessing = 0;
		}};
""")
insert_before("/* Begin PBXVariantGroup section */", f"""/* Begin PBXTargetDependency section */
		{ID['dependency']} /* PBXTargetDependency */ = {{
			isa = PBXTargetDependency;
			target = {ID['target']} /* AirDrawThumbnail */;
			targetProxy = {ID['proxy']} /* PBXContainerItemProxy */;
		}};
/* End PBXTargetDependency section */

""")
insert_before("/* End XCBuildConfiguration section */", config("debug", "debug") + config("release", "release"))
insert_before("/* End XCConfigurationList section */", f"""		{ID['configs']} /* Build configuration list for PBXNativeTarget "AirDrawThumbnail" */ = {{
			isa = XCConfigurationList;
			buildConfigurations = (
				{ID['debug']} /* Debug */,
				{ID['release']} /* Release */,
			);
			defaultConfigurationIsVisible = 0;
			defaultConfigurationName = Release;
		}};
""")
pbx.write_text(s)
print("Added AirDrawThumbnail target to", pbx)
