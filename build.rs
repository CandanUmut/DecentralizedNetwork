fn main() {
    println!("cargo:rustc-link-search=native=C:\\Program Files\\Npcap\\Lib\\x64");
    println!("cargo:rustc-link-lib=Packet"); // Link the Packet.lib file
}
