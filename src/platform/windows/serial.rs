//! 串口列表枚举与项目内部描述转换。
//! 本模块只执行短时设备发现，不打开串口，也不承担持续读写会话。

use crate::model::{AppError, SerialPortDescriptor};

/// 枚举当前系统串口，并保留 serialport 提供的 USB 设备描述。
pub fn query_serial_ports() -> Result<Vec<SerialPortDescriptor>, AppError> {
    let mut ports = serialport::available_ports()
        .map_err(|error| AppError::WindowsApi {
            context: "无法枚举串口".into(),
            code: error.to_string(),
        })?
        .into_iter()
        .map(|port| {
            let mut descriptor = SerialPortDescriptor {
                port_name: port.port_name,
                port_type: "未知".into(),
                manufacturer: None,
                product: None,
                serial_number: None,
                vid: None,
                pid: None,
            };
            match port.port_type {
                serialport::SerialPortType::UsbPort(info) => {
                    descriptor.port_type = "USB".into();
                    descriptor.manufacturer = info.manufacturer;
                    descriptor.product = info.product;
                    descriptor.serial_number = info.serial_number;
                    descriptor.vid = Some(info.vid);
                    descriptor.pid = Some(info.pid);
                }
                serialport::SerialPortType::BluetoothPort => {
                    descriptor.port_type = "蓝牙".into();
                }
                serialport::SerialPortType::PciPort => {
                    descriptor.port_type = "PCI".into();
                }
                serialport::SerialPortType::Unknown => {}
            }
            descriptor
        })
        .collect::<Vec<_>>();
    ports.sort_by(|left, right| left.port_name.cmp(&right.port_name));
    Ok(ports)
}
