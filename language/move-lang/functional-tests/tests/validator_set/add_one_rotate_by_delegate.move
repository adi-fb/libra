// Make bob a validator, set alice as bob's delegate.
// Test that alice can rotate bob's key and invoke reconfiguration.

//! account: alice
//! account: bob, 1000000, 0, validator
//! account: carrol, 1000000, 0, validator

//! sender: bob
script {
use 0x0::ValidatorConfig;
fun main() {
    // register alice as bob's delegate
    ValidatorConfig::set_operator({{alice}});
}
}

// check: EXECUTED

//! new-transaction
//! sender: alice
script {
use 0x0::ValidatorConfig;
// test alice can rotate bob's consensus public key
fun main() {
    0x0::Transaction::assert(ValidatorConfig::get_operator({{bob}}) == {{alice}}, 44);
    ValidatorConfig::set_consensus_pubkey({{bob}}, x"20");

    // check new key is "20"
    let config = ValidatorConfig::get_config({{bob}});
    0x0::Transaction::assert(*ValidatorConfig::get_consensus_pubkey(&config) == x"20", 99);
}
}

// check: EXECUTED

//! new-transaction
//! sender: bob
script {
use 0x0::ValidatorConfig;
// test bob can not rotate his public key because it delegated
fun main() {
    // check initial key was "beefbeef"
    let config = ValidatorConfig::get_config({{bob}});
    0x0::Transaction::assert(*ValidatorConfig::get_consensus_pubkey(&config) == x"20", 99);

    ValidatorConfig::set_consensus_pubkey({{bob}}, x"30");
}
}

// check: ABORTED

//! block-prologue
//! proposer: carrol
//! block-time: 2

// check: EXECUTED

//! new-transaction
//! sender: alice
//! expiration-time: 3
script {
use 0x0::ValidatorConfig;
use 0x0::LibraSystem;
// test alice can invoke reconfiguration upon successful rotation of bob's consensus public key
fun main() {
    ValidatorConfig::set_consensus_pubkey({{bob}}, x"30");

    // call update to reconfigure
    LibraSystem::update_and_reconfigure();

    // check bob's public key is updated
    let validator_config = LibraSystem::get_validator_config({{bob}});
    0x0::Transaction::assert(*ValidatorConfig::get_consensus_pubkey(&validator_config) == x"30", 99);

}
}

// check: EXECUTED
