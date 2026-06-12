use crate::{
    accelerator::KeyAccelerator,
    icon::{BadIcon, Icon, NativeIcon, RgbaIcon},
    items::PredefinedMenuItemType,
    util::{AddOp, Counter},
    IsMenuItem, MenuId, MenuItemKind, MenuItemType,
};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::atomic::{AtomicBool, Ordering},
};

static COUNTER: Counter = Counter::new();

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformIcon(RgbaIcon);

impl PlatformIcon {
    pub fn from_rgba(rgba: Vec<u8>, width: u32, height: u32) -> Result<Self, BadIcon> {
        Ok(Self(RgbaIcon::from_rgba(rgba, width, height)?))
    }
}

pub struct Menu {
    id: MenuId,
    children: Vec<Rc<RefCell<MenuChild>>>,
}

impl Menu {
    pub fn new(id: Option<MenuId>) -> Self {
        Self {
            id: id.unwrap_or_else(|| MenuId(COUNTER.next().to_string())),
            children: Vec::new(),
        }
    }

    pub fn id(&self) -> &MenuId {
        &self.id
    }

    pub fn add_menu_item(&mut self, item: &dyn IsMenuItem, op: AddOp) -> crate::Result<()> {
        match op {
            AddOp::Append => self.children.push(item.child()),
            AddOp::Insert(position) => self.children.insert(position, item.child()),
        }

        Ok(())
    }

    pub fn remove(&mut self, item: &dyn IsMenuItem) -> crate::Result<()> {
        let index = self
            .children
            .iter()
            .position(|child| child.borrow().id == item.id())
            .ok_or(crate::Error::NotAChildOfThisMenu)?;
        self.children.remove(index);
        Ok(())
    }

    pub fn items(&self) -> Vec<MenuItemKind> {
        self.children
            .iter()
            .map(|child| child.borrow().kind(child.clone()))
            .collect()
    }
}

#[derive(Debug, Default)]
pub struct MenuChild {
    item_type: MenuItemType,
    text: String,
    enabled: bool,
    id: MenuId,
    accelerator: Option<KeyAccelerator>,
    checked: Option<Rc<AtomicBool>>,
    icon: Option<Icon>,
    pub children: Option<Vec<Rc<RefCell<MenuChild>>>>,
}

impl MenuChild {
    pub fn new(
        text: &str,
        enabled: bool,
        key_accelerator: Option<KeyAccelerator>,
        id: Option<MenuId>,
    ) -> Self {
        Self {
            text: text.to_string(),
            enabled,
            accelerator: key_accelerator,
            id: id.unwrap_or_else(|| MenuId(COUNTER.next().to_string())),
            item_type: MenuItemType::MenuItem,
            checked: None,
            icon: None,
            children: None,
        }
    }

    pub fn new_submenu(text: &str, enabled: bool, id: Option<MenuId>) -> Self {
        Self {
            text: text.to_string(),
            enabled,
            id: id.unwrap_or_else(|| MenuId(COUNTER.next().to_string())),
            children: Some(Vec::new()),
            item_type: MenuItemType::Submenu,
            accelerator: None,
            checked: None,
            icon: None,
        }
    }

    pub(crate) fn new_predefined(item_type: PredefinedMenuItemType, text: Option<String>) -> Self {
        Self {
            text: text.unwrap_or_else(|| item_type.text().to_string()),
            enabled: true,
            accelerator: item_type.accelerator().map(Into::into),
            id: MenuId(COUNTER.next().to_string()),
            item_type: MenuItemType::Predefined,
            checked: None,
            icon: None,
            children: None,
        }
    }

    pub fn new_check(
        text: &str,
        enabled: bool,
        checked: bool,
        key_accelerator: Option<KeyAccelerator>,
        id: Option<MenuId>,
    ) -> Self {
        Self {
            text: text.to_string(),
            enabled,
            checked: Some(Rc::new(AtomicBool::new(checked))),
            accelerator: key_accelerator,
            id: id.unwrap_or_else(|| MenuId(COUNTER.next().to_string())),
            item_type: MenuItemType::Check,
            children: None,
            icon: None,
        }
    }

    pub fn new_icon(
        text: &str,
        enabled: bool,
        icon: Option<Icon>,
        key_accelerator: Option<KeyAccelerator>,
        id: Option<MenuId>,
    ) -> Self {
        Self {
            text: text.to_string(),
            enabled,
            icon,
            accelerator: key_accelerator,
            id: id.unwrap_or_else(|| MenuId(COUNTER.next().to_string())),
            item_type: MenuItemType::Icon,
            checked: None,
            children: None,
        }
    }

    pub fn new_native_icon(
        text: &str,
        enabled: bool,
        _native_icon: Option<NativeIcon>,
        key_accelerator: Option<KeyAccelerator>,
        id: Option<MenuId>,
    ) -> Self {
        Self {
            text: text.to_string(),
            enabled,
            accelerator: key_accelerator,
            id: id.unwrap_or_else(|| MenuId(COUNTER.next().to_string())),
            item_type: MenuItemType::Icon,
            checked: None,
            children: None,
            icon: None,
        }
    }
}

impl MenuChild {
    pub(crate) fn item_type(&self) -> MenuItemType {
        self.item_type
    }

    pub fn id(&self) -> &MenuId {
        &self.id
    }

    pub fn text(&self) -> String {
        self.text.clone()
    }

    pub fn set_text(&mut self, text: &str) {
        self.text = text.to_string();
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub fn set_key_accelerator(
        &mut self,
        accelerator: Option<KeyAccelerator>,
    ) -> crate::Result<()> {
        self.accelerator = accelerator;
        Ok(())
    }

    pub fn is_checked(&self) -> bool {
        self.checked
            .as_ref()
            .map(|checked| checked.load(Ordering::Relaxed))
            .unwrap_or(false)
    }

    pub fn set_checked(&mut self, checked: bool) {
        if let Some(current) = &self.checked {
            current.store(checked, Ordering::Release);
        }
    }

    pub fn set_icon(&mut self, icon: Option<Icon>) {
        self.icon = icon;
    }

    pub fn add_menu_item(&mut self, item: &dyn IsMenuItem, op: AddOp) -> crate::Result<()> {
        let children = self.children.as_mut().expect("submenu child list");
        match op {
            AddOp::Append => children.push(item.child()),
            AddOp::Insert(position) => children.insert(position, item.child()),
        }

        Ok(())
    }

    pub fn remove(&mut self, item: &dyn IsMenuItem) -> crate::Result<()> {
        let children = self.children.as_mut().expect("submenu child list");
        let index = children
            .iter()
            .position(|child| child.borrow().id == item.id())
            .ok_or(crate::Error::NotAChildOfThisMenu)?;
        children.remove(index);
        Ok(())
    }

    pub fn items(&self) -> Vec<MenuItemKind> {
        self.children
            .as_ref()
            .map(|children| {
                children
                    .iter()
                    .map(|child| child.borrow().kind(child.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }
}
