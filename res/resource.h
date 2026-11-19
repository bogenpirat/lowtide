// Resource IDs shared between app.rc and src/ui/settings.rs (module `ids`). Keep both in sync.
#define IDI_APP                 1
#define IDD_SETTINGS            100

#define IDC_THRESHOLD           1001
#define IDC_THRESHOLD_SPIN      1002
#define IDC_SUPPRESS            1003
#define IDC_POLL                1004
#define IDC_POLL_SPIN           1005
#define IDC_HOTKEY              1006
#define IDC_AUTOSTART           1007
#define IDC_LANGUAGE            1008

#define IDC_ALERT_MSG           1010
#define IDC_ALERT_ANCHOR        1011
#define IDC_ALERT_MARGIN        1012
#define IDC_ALERT_MARGIN_SPIN   1013

#define IDC_FADE                1030
#define IDC_FADE_START          1031
#define IDC_FADE_START_SPIN     1032
#define IDC_FADE_END            1033
#define IDC_FADE_END_SPIN       1034
#define IDC_FADE_CHARGE         1035
#define IDC_FADE_CHARGE_SPIN    1036

#define IDC_INFO_ANCHOR         1020
#define IDC_INFO_MARGIN         1021
#define IDC_INFO_MARGIN_SPIN    1022
#define IDC_INFO_OFFX           1023
#define IDC_INFO_OFFX_SPIN      1024
#define IDC_INFO_OFFY           1025
#define IDC_INFO_OFFY_SPIN      1026
#define IDC_INFO_DURATION       1027
#define IDC_INFO_DURATION_SPIN  1028

// Text style blocks: base + offset (see STYLE_* in settings.rs)
#define IDC_ALERT_STYLE         1100
#define IDC_INFO_STYLE          1200

#define IDC_PREVIEW             1300
#define IDC_APPLY               1301

// Static labels, so they can be translated at runtime
#define IDC_L_GENERAL           1401
#define IDC_L_THRESHOLD         1402
#define IDC_L_POLL              1403
#define IDC_L_SECONDS           1404
#define IDC_L_HOTKEY            1405
#define IDC_L_LANGUAGE          1406
#define IDC_L_ALERT_GROUP       1407
#define IDC_L_MESSAGE           1408
#define IDC_L_PLACEHOLDERS      1409
#define IDC_L_ALERT_POSITION    1410
#define IDC_L_ALERT_MARGIN      1411
#define IDC_L_FADE_DOWN_TO      1412
#define IDC_L_FADE_CHARGE_UNIT  1413
#define IDC_L_FADE_FROM         1414
#define IDC_L_FADE_TO           1415
#define IDC_L_FADE_START_UNIT   1416
#define IDC_L_FADE_END_UNIT     1417
#define IDC_L_INFO_GROUP        1420
#define IDC_L_INFO_POSITION     1421
#define IDC_L_INFO_MARGIN       1422
#define IDC_L_OFFSET_X          1423
#define IDC_L_SHOW_FOR          1424

// Statistics window (src/ui/statistics.rs, module `ids`)
#define IDD_STATISTICS          101
#define IDC_S_DEVICE            2001
#define IDC_S_CURRENT           2002
#define IDC_S_LASTS             2003
#define IDC_S_ALERT             2004
#define IDC_S_FULL              2005
#define IDC_S_BASIS             2006
#define IDC_S_TABLE             2007
#define IDC_S_LAST_CHARGE       2008
#define IDC_S_BETWEEN           2009
#define IDC_S_CHARGE_TIME       2010
#define IDC_S_LOWEST            2011
#define IDC_S_SINCE             2012
#define IDC_S_CHART             2013
#define IDC_S_L_DEVICE          2101
#define IDC_S_L_FORECAST        2102
#define IDC_S_L_USAGE           2103
#define IDC_S_L_CHARGING        2104
#define IDC_S_L_CHART           2105
