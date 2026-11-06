// The showcase's Firefox: a throwaway profile that opens straight on its
// page, dark, with nothing asked and nothing sent.
user_pref("browser.shell.checkDefaultBrowser", false);
user_pref("browser.aboutwelcome.enabled", false);
user_pref("browser.startup.homepage_override.mstone", "ignore");
user_pref("startup.homepage_welcome_url", "");
user_pref("startup.homepage_welcome_url.additional", "");
user_pref("browser.sessionstore.resume_from_crash", false);
user_pref("browser.tabs.warnOnClose", false);
user_pref("browser.tabs.inTitlebar", 1);
user_pref("datareporting.policy.dataSubmissionEnabled", false);
user_pref("datareporting.healthreport.uploadEnabled", false);
user_pref("toolkit.telemetry.reportingpolicy.firstRun", false);
user_pref("app.update.auto", false);
user_pref("app.normandy.enabled", false);
user_pref("extensions.pocket.enabled", false);
user_pref("browser.newtabpage.enabled", false);
user_pref("ui.systemUsesDarkTheme", 1);
user_pref("layout.css.prefers-color-scheme.content-override", 0);
user_pref("browser.theme.toolbar-theme", 0);
user_pref("browser.theme.content-theme", 0);
user_pref("media.autoplay.default", 0);
user_pref("media.hardwaremediakeys.enabled", true);
// Playing, as far as the media session and the bar know, and silent.
user_pref("media.volume_scale", "0.0");
user_pref("sidebar.revamp", false);
user_pref("browser.translations.automaticallyPopup", false);
// No promotions in the toolbar.
user_pref("browser.ipProtection.enabled", false);
user_pref("browser.vpn_promo.enabled", false);
user_pref("browser.toolbars.bookmarks.visibility", "never");
