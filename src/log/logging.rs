// module logging

// logging convenience functions

#[derive(Eq, PartialEq)]
pub enum Level {
    INFO,
    DEBUG,
    TRACE,
    WARN,
}

//#[derive(Default)]
pub struct Logging {
    pub log_level: Level,
}

impl Logging {
    // info
    pub fn info(&self, msg: &str) {
        if self.log_level == Level::INFO
            || self.log_level == Level::DEBUG
            || self.log_level == Level::TRACE
        {
            println!("\x1b[1;94m {} \x1b[0m  : {}", "INFO", msg);
        }
    }

    // debug
    pub fn debug(&self, msg: &str) {
        if self.log_level == Level::DEBUG {
            println!("\x1b[1;92m {} \x1b[0m : {}", "DEBUG", msg);
        }
    }

    // info with highlight
    pub fn hi(&self, msg: &str) {
        if self.log_level == Level::INFO || self.log_level == Level::DEBUG {
            println!("\x1b[1;94m {}  \x1b[0m : \x1b[1;95m{} \x1b[0m", "INFO", msg);
        }
    }

    // info with mid level highlight
    pub fn mid(&self, msg: &str) {
        if self.log_level == Level::INFO || self.log_level == Level::DEBUG {
            println!("\x1b[1;94m {}  \x1b[0m : \x1b[1;94m{} \x1b[0m", "INFO", msg);
        }
    }

    // info with low level highlight
    pub fn lo(&self, msg: &str) {
        if self.log_level == Level::INFO || self.log_level == Level::DEBUG {
            println!("\x1b[1;94m {}  \x1b[0m : \x1b[1;92m{} \x1b[0m", "INFO", msg);
        }
    }

    // info with extra level highlight
    //pub fn ex(&self, msg: &str) {
    //    if self.log_level == Level::INFO || self.log_level == Level::DEBUG {
    //        println!("\x1b[1;94m {}  \x1b[0m : \x1b[1;98m{} \x1b[0m", "INFO", msg);
    //   }
    //}

    // trace
    pub fn trace(&self, msg: &str) {
        if self.log_level == Level::TRACE {
            println!("\x1b[1;96m {} \x1b[0m : \x1b[1;95m{} \x1b[0m", "TRACE", msg);
        }
    }

    // warning
    pub fn warn(&self, msg: &str) {
        if self.log_level == Level::WARN || self.log_level == Level::INFO {
            println!("\x1b[1;93m {} \x1b[0m  : {}", "WARN", msg);
        }
    }

    // error
    pub fn error(&self, msg: &str) {
        println!("\x1b[1;91m {} \x1b[0m : {}", "ERROR", msg);
    }
}
