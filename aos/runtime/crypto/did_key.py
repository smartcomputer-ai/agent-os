import json
from multibase import encode, decode
import base58
import multicodec
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives.serialization import Encoding, PublicFormat, PrivateFormat, NoEncryption


# This creates a simple DID, using the did:key method, with an Ed25519 keypair.
#code taken from here: https://grotto-networking.com/blog/posts/DID_Key.html


def create_did():
    "This creates a simple DID, using the did:key method, with an Ed25519 keypair."
    private_key = Ed25519PrivateKey.generate()
    public_key = private_key.public_key()

    public_key_bytes = public_key.public_bytes(Encoding.Raw, PublicFormat.Raw)
    private_key_bytes = private_key.private_bytes(Encoding.Raw, PrivateFormat.Raw, NoEncryption())

    # Let's encode the public key for use in the did:key method
    public_encoded = encode('base58btc', multicodec.add_prefix('ed25519-pub', public_key_bytes))
    #print(public_encoded)

    multi_pub = public_encoded.decode('utf8')
    did = f"did:key:{multi_pub}"

    return (did, public_key_bytes, private_key_bytes,)

def extract_key_type_and_public_key(did_key:str) -> tuple[str, bytes]:
    """Function to extract the key type and bytes from a DID key"""
    multi_pub = did_key.split(":")[-1]
    ed255_Encoded = multi_pub.encode('utf8')
    ed255_Multi = decode(ed255_Encoded)
    codec = multicodec.get_codec(ed255_Multi)
    ed255_binary:bytes = multicodec.remove_prefix(ed255_Multi)
    return (codec, ed255_binary,)


def create_did_doc(did_key:str):
    """Function to create a DID document from a multibase encoded public key
    
    I'm leaving off the keyAreement stuff for now since I haven't found 
    a python implementation for the Ed25519 to X25519 key derivation yet."""

    multi_pub = did_key.split(":")[-1]

    doc = {
    "@context": [
        "https://www.w3.org/ns/did/v1",
        "https://w3id.org/security/suites/ed25519-2020/v1"
        # "https://w3id.org/security/suites/x25519-2020/v1"
    ],
    "id": "did:key:" + multi_pub,
    "verificationMethod": [{ # Signature verification method
        "id": "did:key:" + multi_pub + "#" + multi_pub,
        "type": "Ed25519VerificationKey2020",
        "controller": "did:key:" + multi_pub,
        "publicKeyMultibase": multi_pub
    }],
    "authentication": [
        "did:key:" + multi_pub + "#" + multi_pub
        ],
    "assertionMethod": [
        "did:key:" + multi_pub + "#" + multi_pub
        ],
    "capabilityDelegation": [
        "did:key:" + multi_pub + "#" + multi_pub
        ],
    "capabilityInvocation": [
        "did:key:" + multi_pub + "#" + multi_pub
        ],
    # "keyAgreement": [{
    #     "id": "did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK#z6LSj72tK8brWgZja8NLRwPigth2T9QRiG1uH9oKZuKjdh9p",
    #     "type": "X25519KeyAgreementKey2020",
    #     "controller": "did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK",
    #     "publicKeyMultibase": "z6LSj72tK8brWgZja8NLRwPigth2T9QRiG1uH9oKZuKjdh9p"
    # }]
    }
    return doc

if __name__ == "__main__":
    did, public_key_bytes, private_key_bytes = create_did()
    print('Public Key:')
    print(public_key_bytes.hex())
    # Danger: in real life you don't flaunt your private key like this!
    print('Private Key:')
    print(private_key_bytes.hex()) # For education purposes only!!!
    print("DID:", did)

    print("================================")
    type, public_key_bytes2 = extract_key_type_and_public_key(did)
    print("Type:", type)
    print('Public Key:', public_key_bytes2.hex())

    print("================================")
    did_doc_dict = create_did_doc(did)
    print(json.dumps(did_doc_dict, indent=2))
