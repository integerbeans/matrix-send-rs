use std::borrow::Cow;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::PathBuf;

use crate::{matrix::MatrixClient, Error, Result};

use atty::Stream;

use structopt::{
    clap::{arg_enum, ArgGroup},
    StructOpt,
};

use matrix_sdk::{
    room::Room,
    ruma::events::room::message::{
        EmoteMessageEventContent, MessageEventContent, MessageType, NoticeMessageEventContent,
        TextMessageEventContent,
    },
    ruma::identifiers::{RoomId, RoomIdOrAliasId, ServerName},
};

use mime::Mime;

mod user;

#[derive(Debug, StructOpt)]
pub(crate) enum Command {
    /// Join Room
    Join(JoinCommand),

    /// Leave Room
    Leave(LeaveCommand),

    /// Send Message into Room
    Send(SendCommand),

    /// List Rooms
    List(ListCommand),

    /// User commands for room
    User(UserCommand),

    /// Send file into room
    SendFile(SendFileCommand),
}

impl Command {
    pub(super) async fn run(self, client: MatrixClient) -> Result {
        match self {
            Self::Join(command) => command.run(client).await,
            Self::List(command) => command.run(client).await,
            Self::Send(command) => command.run(client).await,
            Self::Leave(command) => command.run(client).await,
            Self::User(command) => command.run(client).await,
            Self::SendFile(command) => command.run(client).await,
        }
    }
}

#[derive(Debug, StructOpt)]
pub(crate) struct JoinCommand {
    /// Alias or ID of Room
    room: RoomIdOrAliasId,

    /// Homeservers used to find the Room
    servers: Vec<Box<ServerName>>,
}

impl JoinCommand {
    async fn run(self, client: MatrixClient) -> Result {
        client
            .join_room_by_id_or_alias(&self.room, &self.servers)
            .await?;
        Ok(())
    }
}

#[derive(Debug, StructOpt)]
pub(crate) struct LeaveCommand {
    /// Room ID
    room: RoomId,
}

impl LeaveCommand {
    async fn run(self, client: MatrixClient) -> Result {
        client.joined_room(&self.room)?.leave().await?;
        Ok(())
    }
}

#[derive(Debug, StructOpt)]
#[structopt(
    group = ArgGroup::with_name("msgopt"),
    group = ArgGroup::with_name("format"),
    group = ArgGroup::with_name("type"),
)]
pub(crate) struct SendCommand {
    /// Room ID
    room: RoomId,

    /// Message to send
    #[structopt(group = "msgopt")]
    message: Option<String>,

    /// Read Message from file
    #[structopt(short, long, group = "msgopt")]
    file: Option<PathBuf>,

    /// Put message in code block
    #[structopt(name = "language", long = "code", group = "format")]
    code: Option<Option<String>>,

    /// Message is Markdown
    #[structopt(long, group = "format")]
    markdown: bool,

    /// Send notice
    #[structopt(long, group = "type")]
    notice: bool,

    /// Send emote
    #[structopt(long, group = "type")]
    emote: bool,
}

impl SendCommand {
    async fn run(self, client: MatrixClient) -> Result {
        let msg = if let Some(msg) = self.message {
            msg
        } else if let Some(file) = self.file {
            fs::read_to_string(file)?
        } else {
            let mut line = String::new();
            if atty::is(Stream::Stdin) {
                println!("Message:");
                io::stdin().read_line(&mut line)?;
            } else {
                io::stdin().read_to_string(&mut line)?;
            }
            line
        };
        let (msg, markdown) = if let Some(language) = self.code {
            let mut fmt_msg = String::from("```");
            if let Some(language) = language {
                fmt_msg.push_str(&language);
            }
            fmt_msg.push('\n');
            fmt_msg.push_str(&msg);
            if !fmt_msg.ends_with('\n') {
                fmt_msg.push('\n');
            }
            fmt_msg.push_str("```");
            (fmt_msg, true)
        } else {
            (msg, self.markdown)
        };
        let content = if self.notice {
            MessageType::Notice(if markdown {
                NoticeMessageEventContent::markdown(msg)
            } else {
                NoticeMessageEventContent::plain(msg)
            })
        } else if self.emote {
            MessageType::Emote(if markdown {
                EmoteMessageEventContent::markdown(msg)
            } else {
                EmoteMessageEventContent::plain(msg)
            })
        } else {
            MessageType::Text(if markdown {
                TextMessageEventContent::markdown(msg)
            } else {
                TextMessageEventContent::plain(msg)
            })
        };
        client
            .joined_room(&self.room)?
            .send(MessageEventContent::new(content), None)
            .await?;
        Ok(())
    }
}

#[derive(Debug, StructOpt)]
pub(crate) struct ListCommand {
    /// Kind
    #[structopt(possible_values = &["all", "joined", "invited", "left"], default_value = "joined")]
    kind: Vec<Kind>,
}

arg_enum! {
    #[derive(Debug)]
    enum Kind {
        All,
        Joined,
        Invited,
        Left
    }
}

impl ListCommand {
    async fn run(self, client: MatrixClient) -> Result {
        for room in client.rooms().into_iter().filter(|r| {
            self.kind.iter().any(|k| {
                matches!(
                    (k, r),
                    (Kind::All, _)
                        | (Kind::Joined, Room::Joined(_))
                        | (Kind::Left, Room::Left(_))
                        | (Kind::Invited, Room::Invited(_))
                )
            })
        }) {
            if let Ok(name) = room.display_name().await {
                println!("{}\t{}", room.room_id(), name);
            } else {
                println!("{}", room.room_id());
            }
        }
        Ok(())
    }
}

#[derive(Debug, StructOpt)]
pub(crate) struct UserCommand {
    /// Room ID
    room: RoomId,

    #[structopt(subcommand)]
    command: user::Command,
}

impl UserCommand {
    async fn run(self, client: MatrixClient) -> Result {
        self.command.run(client, self.room).await
    }
}

#[derive(Debug, StructOpt)]
pub(crate) struct SendFileCommand {
    /// Room ID
    room: RoomId,

    /// File Path
    file: PathBuf,

    /// Override auto detected mime type
    #[structopt(long)]
    mime: Option<Mime>,

    /// Override fallback text (Defaults to filename)
    #[structopt(long)]
    text: Option<String>,
}

impl SendFileCommand {
    async fn run(self, client: MatrixClient) -> Result {
        client
            .joined_room(&self.room)?
            .send_attachment(
                self.text
                    .as_ref()
                    .map(Cow::from)
                    .or_else(|| self.file.file_name().as_ref().map(|o| o.to_string_lossy()))
                    .ok_or(Error::InvalidFile)?
                    .as_ref(),
                self.mime.as_ref().unwrap_or(
                    &mime_guess::from_path(&self.file).first_or(mime::APPLICATION_OCTET_STREAM),
                ),
                &mut File::open(&self.file)?,
                None,
            )
            .await?;
        Ok(())
    }
}
