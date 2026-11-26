import Foundation
import Security

enum PairingKeys {
    private static func query(_ id: Data) -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: "com.usethatscreen.ipad.pairing",
            kSecAttrAccount as String: id.base64EncodedString(),
        ]
    }

    static func secret(for id: Data) -> Data? {
        var attributes = query(id)
        attributes[kSecReturnData as String] = true
        var result: CFTypeRef?
        guard SecItemCopyMatching(attributes as CFDictionary, &result) == errSecSuccess else { return nil }
        return result as? Data
    }

    static func save(_ secret: Data, for id: Data) -> Bool {
        let key = query(id)
        let value = [kSecValueData as String: secret]
        let updated = SecItemUpdate(key as CFDictionary, value as CFDictionary)
        if updated == errSecSuccess { return true }
        guard updated == errSecItemNotFound else { return false }
        var attributes = key.merging(value) { _, second in second }
        attributes[kSecAttrAccessible as String] = kSecAttrAccessibleWhenUnlockedThisDeviceOnly
        return SecItemAdd(attributes as CFDictionary, nil) == errSecSuccess
    }
}
