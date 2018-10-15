mod ffi;

use std::{
    sync::{Once, ONCE_INIT},
    ffi::{CString},
};

use objc_id::{Id, Shared};
use objc::{msg_send, sel, sel_impl, class, runtime::{Class, Object, Protocol, Sel, BOOL, YES, NO}, declare::ClassDecl};
use objc_foundation::{NSObject, NSDictionary, INSDictionary, NSString, INSString, NSArray, INSArray, NSData, INSData};

use uuid::Uuid;

use ffi::{
    nil,
    dispatch_queue_create,
    DISPATCH_QUEUE_SERIAL,
    CBAdvertisementDataServiceUUIDsKey,
    CBAdvertisementDataLocalNameKey,
    CBManagerState,
    CBCharacteristicProperties,
    CBAttributePermissions,
    CBATTError,
};

use super::super::gatt::{
    primary_service::PrimaryService,
    characteristic::Property,
};

fn objc_to_rust_bool(objc_bool: BOOL) -> bool {
    match objc_bool {
        YES => true,
        NO => false,
        _ => panic!("Unknown Objective-C BOOL value."),
    }
}

static REGISTER_DELEGATE_CLASS: Once = ONCE_INIT;
const PERIPHERAL_MANAGER_DELEGATE_CLASS_NAME: &str = "PeripheralManagerDelegate";
const PERIPHERAL_MANAGER_IVAR: &str = "peripheralManager";
const POWERED_ON_IVAR: &str = "poweredOn";

#[derive(Debug)]
pub struct Peripheral {
    peripheral_manager_delegate: Id<Object, Shared>,
}

impl Peripheral {
    pub fn new() -> Self {
        REGISTER_DELEGATE_CLASS.call_once(|| {
            let mut decl = ClassDecl::new(PERIPHERAL_MANAGER_DELEGATE_CLASS_NAME, class!(NSObject)).unwrap();
            decl.add_protocol(Protocol::get("CBPeripheralManagerDelegate").unwrap());

            decl.add_ivar::<*mut Object>(PERIPHERAL_MANAGER_IVAR);
            decl.add_ivar::<*mut Object>(POWERED_ON_IVAR);

            unsafe {
                decl.add_method(sel!(init), init as extern fn(&mut Object, Sel) -> *mut Object);
                decl.add_method(sel!(peripheralManagerDidUpdateState:), peripheral_manager_did_update_state as extern fn(&mut Object, Sel, *mut Object));
                decl.add_method(sel!(peripheralManagerDidStartAdvertising:error:), peripheral_manager_did_start_advertising_error as extern fn(&mut Object, Sel, *mut Object, *mut Object));
                decl.add_method(sel!(peripheralManager:didAddService:error:), peripheral_manager_did_add_service_error as extern fn(&mut Object, Sel, *mut Object, *mut Object, *mut Object));
                decl.add_method(sel!(peripheralManager:didReceiveReadRequest:), peripheral_manager_did_receive_read_request as extern fn(&mut Object, Sel, *mut Object, *mut Object));
                decl.add_method(sel!(peripheralManager:didReceiveWriteRequests:), peripheral_manager_did_receive_write_requests as extern fn(&mut Object, Sel, *mut Object, *mut Object));
            }

            decl.register();
        });

        let peripheral_manager_delegate = unsafe {
            let cls = Class::get(PERIPHERAL_MANAGER_DELEGATE_CLASS_NAME).unwrap();
            let mut obj: *mut Object = msg_send![cls, alloc];
            obj = msg_send![obj, init];
            Id::from_ptr(obj).share()
        };

        Peripheral {
            peripheral_manager_delegate,
        }
    }

    pub fn is_powered_on(self: &Self) -> bool {
        objc_to_rust_bool(
            unsafe {
                *self.peripheral_manager_delegate.get_ivar::<*mut Object>(POWERED_ON_IVAR) as i8
            }
        )
    }

    pub fn start_advertising(self: &Self, name: &str, uuids: &[Uuid]) {
        let peripheral_manager = unsafe {
            *self.peripheral_manager_delegate.get_ivar::<*mut Object>(PERIPHERAL_MANAGER_IVAR)
        };

        let mut keys: Vec<&NSString> = vec![];
        let mut objects: Vec<Id<NSObject>> = vec![];

        unsafe {
            keys.push(&*(CBAdvertisementDataLocalNameKey as *mut NSString));
            objects.push(Id::from_retained_ptr(msg_send![NSString::from_str(name), copy]));
            keys.push(&*(CBAdvertisementDataServiceUUIDsKey as *mut NSString));
            objects.push(
                Id::from_retained_ptr(
                    msg_send![
                        NSArray::from_vec(
                            uuids
                                .iter().map(|u| {
                                    NSString::from_str(&u.to_hyphenated().to_string())
                                })
                                .collect::<Vec<Id<NSString>>>()
                        ),
                        copy
                    ]
                )
            );
        }

        let advertising_data = NSDictionary::from_keys_and_objects(keys.as_slice(), objects);
        unsafe { msg_send![peripheral_manager, startAdvertising:advertising_data]; }
    }

    pub fn stop_advertising(self: &Self) {
        unsafe {
            let peripheral_manager = *self.peripheral_manager_delegate.get_ivar::<*mut Object>(PERIPHERAL_MANAGER_IVAR);
            msg_send![peripheral_manager, stopAdvertising];
        }
    }

    pub fn is_advertising(self: &Self) -> bool {
        unsafe {
            let peripheral_manager = *self.peripheral_manager_delegate.get_ivar::<*mut Object>(PERIPHERAL_MANAGER_IVAR);
            objc_to_rust_bool(msg_send![peripheral_manager, isAdvertising])
        }
    }

    pub fn add_service(self: &Self, primary_service: &PrimaryService) {
        let characteristics: Vec<Id<NSObject>> = primary_service
            .characteristics
            .iter()
            .map(
                |characteristic| {
                    let mut properties = 0x000;
                    let mut permissions = 0x000;

                    if characteristic.properties.contains(&Property::Read) {
                      properties |= CBCharacteristicProperties::CBCharacteristicPropertyRead as u8;

                      if characteristic.secure.contains(&Property::Read) {
                        permissions |= CBAttributePermissions::CBAttributePermissionsReadEncryptionRequired as u8;
                      } else {
                        permissions |= CBAttributePermissions::CBAttributePermissionsReadable as u8;
                      }
                    }

                    if characteristic.properties.contains(&Property::WriteWithoutResponse) {
                      properties |= CBCharacteristicProperties::CBCharacteristicPropertyWriteWithoutResponse as u8;

                      if characteristic.secure.contains(&Property::WriteWithoutResponse) {
                        permissions |= CBAttributePermissions::CBAttributePermissionsWriteEncryptionRequired as u8;
                      } else {
                        permissions |= CBAttributePermissions::CBAttributePermissionsWriteable as u8;
                      }
                    }

                    if characteristic.properties.contains(&Property::Write) {
                      properties |= CBCharacteristicProperties::CBCharacteristicPropertyWrite as u8;

                      if characteristic.secure.contains(&Property::Write) {
                        permissions |= CBAttributePermissions::CBAttributePermissionsWriteEncryptionRequired as u8;
                      } else {
                        permissions |= CBAttributePermissions::CBAttributePermissionsWriteable as u8;
                      }
                    }

                    if characteristic.properties.contains(&Property::Notify) {
                      if characteristic.secure.contains(&Property::Notify) {
                        properties |= CBCharacteristicProperties::CBCharacteristicPropertyNotifyEncryptionRequired as u8;
                      } else {
                        properties |= CBCharacteristicProperties::CBCharacteristicPropertyNotify as u8;
                      }
                    }

                    if characteristic.properties.contains(&Property::Indicate) {
                      if characteristic.secure.contains(&Property::Indicate) {
                        properties |= CBCharacteristicProperties::CBCharacteristicPropertyIndicateEncryptionRequired as u8;
                      } else {
                        properties |= CBCharacteristicProperties::CBCharacteristicPropertyIndicate as u8;
                      }
                    }

                    unsafe {
                        let init_with_type = NSString::from_str(&characteristic.uuid.to_string());

                        let cls = class!(CBMutableCharacteristic);
                        let obj: *mut Object = msg_send![cls, alloc];

                        let mutable_characteristic: *mut Object = match characteristic.value {
                            Some(ref value) => {
                                msg_send![obj, initWithType:init_with_type
                                                 properties:properties
                                                      value:NSData::with_bytes(value)
                                                permissions:permissions]
                            },
                            None => {
                                msg_send![obj, initWithType:init_with_type
                                                 properties:properties
                                                      value:nil
                                                permissions:permissions]
                            },
                        };

                        Id::from_ptr(mutable_characteristic as *mut NSObject)
                    }
                }
            )
            .collect();

        unsafe {
            let cls = class!(CBMutableService);
            let obj: *mut Object = msg_send![cls, alloc];
            let service: *mut Object = msg_send![obj, initWithType:NSString::from_str(&primary_service.uuid.to_string())
                                                           primary:YES];
            msg_send![service, setValue:NSArray::from_vec(characteristics)
                                 forKey:NSString::from_str("characteristics")];
            msg_send![self.peripheral_manager_delegate, addService:service];
        }
    }
}

impl Default for Peripheral {
    fn default() -> Self {
        Peripheral::new()
    }
}

extern fn init(delegate: &mut Object, _cmd: Sel) -> *mut Object {
    unsafe {
        let cls = class!(CBPeripheralManager);
        let mut obj: *mut Object = msg_send![cls, alloc];

        #[allow(clippy::cast_ptr_alignment)]
        let init_with_delegate = delegate as *mut Object as *mut *mut Object;

        let label = CString::new("CBqueue").unwrap();
        let queue = dispatch_queue_create(label.as_ptr(), DISPATCH_QUEUE_SERIAL);

        obj = msg_send![obj, initWithDelegate:init_with_delegate
                                        queue:queue];
        delegate.set_ivar::<*mut Object>(PERIPHERAL_MANAGER_IVAR, obj);

        delegate.set_ivar::<*mut Object>(POWERED_ON_IVAR, NO as *mut Object);

        delegate
    }
}

// TODO: Implement event stream for all below callback

extern fn peripheral_manager_did_update_state(delegate: &mut Object, _cmd: Sel, peripheral: *mut Object) {
    println!("peripheral_manager_did_update_state");

    unsafe {
        let state: CBManagerState = msg_send![peripheral, state];
        match state {
            CBManagerState::CBManagerStateUnknown => {
                println!("CBManagerStateUnknown");
            },
            CBManagerState::CBManagerStateResetting => {
                println!("CBManagerStateResetting");
            },
            CBManagerState::CBManagerStateUnsupported => {
                println!("CBManagerStateUnsupported");
            },
            CBManagerState::CBManagerStateUnauthorized => {
                println!("CBManagerStateUnauthorized");
            },
            CBManagerState::CBManagerStatePoweredOff => {
                println!("CBManagerStatePoweredOff");
                delegate.set_ivar::<*mut Object>(POWERED_ON_IVAR, NO as *mut Object);
            },
            CBManagerState::CBManagerStatePoweredOn => {
                println!("CBManagerStatePoweredOn");
                delegate.set_ivar::<*mut Object>(POWERED_ON_IVAR, YES as *mut Object);
            },
        };
    }
}

extern fn peripheral_manager_did_start_advertising_error(_delegate: &mut Object, _cmd: Sel, _peripheral: *mut Object, error: *mut Object) {
    println!("peripheral_manager_did_start_advertising_error");
    if objc_to_rust_bool(error as BOOL) {
        let localized_description: *mut Object = unsafe { msg_send![error, localizedDescription] };
        let string = localized_description as *mut NSString;
        println!("{:?}", unsafe { (*string).as_str() });
    }
}

extern fn peripheral_manager_did_add_service_error(_delegate: &mut Object, _cmd: Sel, _peripheral: *mut Object, _service: *mut Object, error: *mut Object) {
    println!("peripheral_manager_did_add_service_error");
    if objc_to_rust_bool(error as BOOL) {
        let localized_description: *mut Object = unsafe { msg_send![error, localizedDescription] };
        let string = localized_description as *mut NSString;
        println!("{:?}", unsafe { (*string).as_str() });
    }
}

extern fn peripheral_manager_did_receive_read_request(_delegate: &mut Object, _cmd: Sel, peripheral: *mut Object, request: *mut Object) {
    unsafe {
        msg_send![peripheral, respondToRequest:request
                                    withResult:CBATTError::CBATTErrorSuccess];
    }
}

extern fn peripheral_manager_did_receive_write_requests(_delegate: &mut Object, _cmd: Sel, peripheral: *mut Object, requests: *mut Object) {
    unsafe {
        for request in (*(requests as *mut NSArray<NSObject>)).to_vec() {
            msg_send![peripheral, respondToRequest:request
                                        withResult:CBATTError::CBATTErrorSuccess];
        }
    }
}