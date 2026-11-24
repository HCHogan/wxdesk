//! The accessibility bus: objects, their properties and their events.

use anyhow::{Context, Result, anyhow};
use atspi_common::{CoordType, ObjectRefOwned, Role};
use atspi_proxies::accessible::AccessibleProxy;
use atspi_proxies::bus::BusProxy;
use atspi_proxies::component::ComponentProxy;
use atspi_proxies::registry::RegistryProxy;
use futures_util::StreamExt;
use std::collections::VecDeque;
use zbus::proxy::CacheProperties;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};
use zbus::{Connection, MatchRule, MessageStream};

/// An object on the bus.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Obj {
    pub bus: String,
    pub path: String,
}

impl Obj {
    fn from_ref(r: &ObjectRefOwned) -> Option<Self> {
        Some(Self {
            bus: r.name_as_str()?.to_owned(),
            path: r.path_as_str().to_owned(),
        })
    }
}

/// Where something is on the screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn center(self) -> (i32, i32) {
        (self.x + self.w / 2, self.y + self.h / 2)
    }

    pub fn is_empty(self) -> bool {
        self.w <= 0 || self.h <= 0
    }
}

#[derive(Clone)]
pub struct A11y {
    pub conn: Connection,
}

/// An event an object sent: `StateChanged` or `PropertyChange`, and of what.
#[derive(Debug)]
pub struct Signal {
    pub source: Obj,
    pub member: String,
    pub kind: String,
    pub detail1: i32,
}

impl A11y {
    /// Connects to the accessibility bus the session bus names.
    pub async fn connect() -> Result<Self> {
        let session = Connection::session().await.context("the session bus")?;
        let address = BusProxy::new(&session).await?.get_address().await?;
        let conn = zbus::connection::Builder::address(address.as_str())?
            .build()
            .await
            .context("the accessibility bus")?;
        Ok(Self { conn })
    }

    async fn acc(&self, o: &Obj) -> Result<AccessibleProxy<'static>> {
        Ok(AccessibleProxy::builder(&self.conn)
            .destination(o.bus.clone())?
            .path(o.path.clone())?
            .cache_properties(CacheProperties::No)
            .build()
            .await?)
    }

    pub async fn name(&self, o: &Obj) -> Result<String> {
        Ok(self.acc(o).await?.name().await?)
    }

    pub async fn role(&self, o: &Obj) -> Result<Role> {
        Ok(self.acc(o).await?.get_role().await?)
    }

    pub async fn parent(&self, o: &Obj) -> Result<Option<Obj>> {
        Ok(Obj::from_ref(&self.acc(o).await?.parent().await?))
    }

    pub async fn children(&self, o: &Obj) -> Result<Vec<Obj>> {
        let children = self.acc(o).await?.get_children().await?;
        Ok(children.iter().filter_map(Obj::from_ref).collect())
    }

    pub async fn extents(&self, o: &Obj) -> Result<Rect> {
        let c = ComponentProxy::builder(&self.conn)
            .destination(o.bus.clone())?
            .path(o.path.clone())?
            .cache_properties(CacheProperties::No)
            .build()
            .await?;
        let (x, y, w, h) = c.get_extents(CoordType::Screen).await?;
        Ok(Rect { x, y, w, h })
    }

    /// The application called `name`.
    pub async fn app(&self, name: &str) -> Result<Obj> {
        let root = Obj {
            bus: "org.a11y.atspi.Registry".into(),
            path: "/org/a11y/atspi/accessible/root".into(),
        };
        for app in self.children(&root).await? {
            if self.name(&app).await.is_ok_and(|n| n == name) {
                return Ok(app);
            }
        }
        Err(anyhow!("no application {name} on the accessibility bus"))
    }

    /// The first object under `root`, breadth first, that `want` takes
    /// by its role and name; at most `limit` objects looked at.
    pub async fn find(
        &self,
        root: &Obj,
        limit: usize,
        want: impl Fn(Role, &str) -> bool,
    ) -> Option<Obj> {
        let mut queue = VecDeque::from([root.clone()]);
        let mut seen = 0;
        while let Some(o) = queue.pop_front() {
            seen += 1;
            if seen > limit {
                return None;
            }
            let (Ok(role), Ok(name)) = (self.role(&o).await, self.name(&o).await) else {
                continue;
            };
            if want(role, &name) {
                return Some(o);
            }
            if let Ok(children) = self.children(&o).await {
                queue.extend(children);
            }
        }
        None
    }

    /// Asks applications for these events.
    pub async fn register(&self, events: &[&str]) -> Result<()> {
        let registry = RegistryProxy::new(&self.conn).await?;
        for e in events {
            registry.register_event(e).await?;
        }
        Ok(())
    }

    /// The object events on the bus, as they come.
    pub async fn signals(&self) -> Result<impl futures_util::Stream<Item = Signal> + use<>> {
        let rule = MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface("org.a11y.atspi.Event.Object")?
            .build();
        let stream = MessageStream::for_match_rule(rule, &self.conn, Some(4096)).await?;
        Ok(stream.filter_map(|m| async move {
            let m = m.ok()?;
            let header = m.header();
            let source = Obj {
                bus: header.sender()?.to_string(),
                path: header.path()?.to_string(),
            };
            let member = header.member()?.to_string();
            let body = m.body();
            let (kind, detail1) = body
                .deserialize::<(String, i32, i32, OwnedValue, (String, OwnedObjectPath))>()
                .map(|b| (b.0, b.1))
                .or_else(|_| {
                    body.deserialize::<(
                        String,
                        i32,
                        i32,
                        OwnedValue,
                        std::collections::HashMap<String, OwnedValue>,
                    )>()
                    .map(|b| (b.0, b.1))
                })
                .ok()?;
            Some(Signal {
                source,
                member,
                kind,
                detail1,
            })
        }))
    }
}
