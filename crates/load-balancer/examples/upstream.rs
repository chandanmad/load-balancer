use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "upstream", long_about = None)]
struct Args {
    #[arg(short, long)]
    port: i32,

    #[arg(short, long)]
    ip: String,
}

fn main() {
    let args = Args::parse();
    println!("{:?}", args);
}
