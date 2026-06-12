/// Map a UIA `ControlTypeId` to a human-readable role string.
///
/// The numeric values are from the Windows SDK header `UIAutomationClient.h`.
/// We keep strings consistent with macOS AXRole naming where possible and
/// use Windows-native names otherwise.
pub(super) fn control_type_to_role(control_type_id: i32) -> &'static str {
    // UIA_*ControlTypeId values (from UIAutomationClient.h)
    const UIA_BUTTON: i32 = 50000;
    const UIA_CALENDAR: i32 = 50001;
    const UIA_CHECKBOX: i32 = 50002;
    const UIA_COMBOBOX: i32 = 50003;
    const UIA_EDIT: i32 = 50004;
    const UIA_HYPERLINK: i32 = 50005;
    const UIA_IMAGE: i32 = 50006;
    const UIA_LISTITEM: i32 = 50007;
    const UIA_LIST: i32 = 50008;
    const UIA_MENU: i32 = 50009;
    const UIA_MENUBAR: i32 = 50010;
    const UIA_MENUITEM: i32 = 50011;
    const UIA_PROGRESSBAR: i32 = 50012;
    const UIA_RADIOBUTTON: i32 = 50013;
    const UIA_SCROLLBAR: i32 = 50014;
    const UIA_SLIDER: i32 = 50015;
    const UIA_SPINNER: i32 = 50016;
    const UIA_STATUSBAR: i32 = 50017;
    const UIA_TAB: i32 = 50018;
    const UIA_TABITEM: i32 = 50019;
    const UIA_TEXT: i32 = 50020;
    const UIA_TOOLBAR: i32 = 50021;
    const UIA_TOOLTIP: i32 = 50022;
    const UIA_TREE: i32 = 50023;
    const UIA_TREEITEM: i32 = 50024;
    const UIA_CUSTOM: i32 = 50025;
    const UIA_GROUP: i32 = 50026;
    const UIA_THUMB: i32 = 50027;
    const UIA_DATAGRID: i32 = 50028;
    const UIA_DATAITEM: i32 = 50029;
    const UIA_DOCUMENT: i32 = 50030;
    const UIA_SPLITBUTTON: i32 = 50031;
    const UIA_WINDOW: i32 = 50032;
    const UIA_PANE: i32 = 50033;
    const UIA_HEADER: i32 = 50034;
    const UIA_HEADERITEM: i32 = 50035;
    const UIA_TABLE: i32 = 50036;
    const UIA_TITLEBAR: i32 = 50037;
    const UIA_SEPARATOR: i32 = 50038;

    match control_type_id {
        UIA_BUTTON => "Button",
        UIA_CALENDAR => "Calendar",
        UIA_CHECKBOX => "CheckBox",
        UIA_COMBOBOX => "ComboBox",
        UIA_EDIT => "Edit",
        UIA_HYPERLINK => "Hyperlink",
        UIA_IMAGE => "Image",
        UIA_LISTITEM => "ListItem",
        UIA_LIST => "List",
        UIA_MENU => "Menu",
        UIA_MENUBAR => "MenuBar",
        UIA_MENUITEM => "MenuItem",
        UIA_PROGRESSBAR => "ProgressBar",
        UIA_RADIOBUTTON => "RadioButton",
        UIA_SCROLLBAR => "ScrollBar",
        UIA_SLIDER => "Slider",
        UIA_SPINNER => "Spinner",
        UIA_STATUSBAR => "StatusBar",
        UIA_TAB => "Tab",
        UIA_TABITEM => "TabItem",
        UIA_TEXT => "Text",
        UIA_TOOLBAR => "ToolBar",
        UIA_TOOLTIP => "ToolTip",
        UIA_TREE => "Tree",
        UIA_TREEITEM => "TreeItem",
        UIA_CUSTOM => "Custom",
        UIA_GROUP => "Group",
        UIA_THUMB => "Thumb",
        UIA_DATAGRID => "DataGrid",
        UIA_DATAITEM => "DataItem",
        UIA_DOCUMENT => "Document",
        UIA_SPLITBUTTON => "SplitButton",
        UIA_WINDOW => "Window",
        UIA_PANE => "Pane",
        UIA_HEADER => "Header",
        UIA_HEADERITEM => "HeaderItem",
        UIA_TABLE => "Table",
        UIA_TITLEBAR => "TitleBar",
        UIA_SEPARATOR => "Separator",
        _ => "Unknown",
    }
}
