use serde::{Deserialize, Serialize};

/// One project and all of its content and metadata.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Project {
    pub name: String,
    pub url: String,
    pub description: String,
    pub content: Content,
    pub thumbnail: String,
    pub skills: Skills,
    /// The priority of this project, used for sorting.
    pub priority: i32,
}

/// The content of a project, including several `Section`s.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Content {
    #[serde(rename = "$value", default)]
    sections: Vec<Section>,
}

/// A section of a project, such as a general-purpose `Section::Section` of content or a special `Section::Criteria` section listing design criteria.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Section {
    /// A generic section, consisting of an optional title and some content.
    Section {
        title: Option<String>,
        #[serde(rename = "$value")]
        content: Vec<Element>,
    },
    /// A section listing design criteria, consisting of an optional title and a list of criteria, each containing a title and description.
    Criteria {
        title: Option<String>,
        #[serde(rename = "item")]
        items: Vec<TitleDesc>,
    },
}

/// A paired title and description.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TitleDesc {
    pub title: String,
    pub description: Text,
}

/// A list of skills, each a string.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Skills {
    #[serde(rename = "skill", default)]
    pub skills: Vec<String>,
}

/// A single element of content, either a `Group` of elements or a `Paragraph` of text.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Element {
    #[serde(rename = "g")]
    Group {
        #[serde(rename = "$value")]
        content: Vec<Element>,
    },
    #[serde(rename = "p")]
    Paragraph(Text),
    #[serde(rename = "img")]
    Image {
        #[serde(rename = "@src")]
        src: String,
        #[serde(rename = "@alt")]
        alt: String,
        caption: Option<Text>,
    },
}

/// Some text, consisting of several `TextElement`s.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Text {
    #[serde(rename = "$value", default)]
    text: Vec<TextElement>,
}

/// A single element of text, a piece of text or hypertext (such as a link).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum TextElement {
    #[serde(rename = "a")]
    Link {
        #[serde(rename = "@href")]
        href: String,
        #[serde(rename = "$value")]
        text: Vec<TextElement>,
    },
    #[serde(rename = "$text")]
    Text(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_serialize() {
        let project = test_project();
        // println!("{}", quick_xml::se::to_string(&project).unwrap());
        println!("{}", serde_json::to_string_pretty(&project).unwrap());
    }

    #[test]
    fn test_deserialize() {
        let test = r#"
        <?xml version="1.0" encoding="UTF-8"?>
        <project>
        <name>Garmin Testing Device</name>
        <url>garmin</url>
        <description>Underwater testing device for Garmin dive watches and air tank sensors, controlled automatically using a web app.</description>
        <date>2022.12-2023.05</date>
        <skills>
            <skill>Embedded Development (ESP-IDF)</skill>
            <skill>Frontend Web Development</skill>
            <skill>PCB Design</skill>
            <skill>Mechatronics</skill>
            <skill>Control Systems</skill>
        </skills>
        <content>
            <section>
                <title>Overview</title>
                <g>
                    <p>Garmin's Sonar Engineering Team creates several dive watches and air tank sensors designed to communicate with each other using their SubWave Sonar technology while underwater. Until recently, to test and calibrate these precise devices, Garmin attached them to a boat using 12 ft PVC poles, leading to several feet of deflection during testing and decreasing both accuracy and repeatability of results.</p>
                    <p>To make the testing and calibration process more stable, accurate, and consistent, I worked with a team of engineers in my EGR 102 class to design and build a new mounting system that would hold the dive watches and air tank sensors fixed in position underwater, while being controllable using a web app to set the exact depth and angular position of the devices being tested.</p>
                    <p>As electrical team lead, I personally designed the mechatronic control system, from the motors used to physically move the testing rig to the UX of the web app used by engineers while performing tests, as well as all the microcontrollers and firmware in between.</p>
                </g>
            </section>
            <criteria>
                <title>Design Criteria</title>
                <item>
                    <title>Accuracy</title>
                    <description>Control system must move devices to within 2" of the desired depth and 5° of the desired angle, withstanding forces from the weight of the device and underwater currents</description>
                </item>
                <item>
                    <title>Durability</title>
                    <description>The testing system will be stored outdoors on a boat and be operated underwater, but still must be built to last without significant maintainance.</description>
                </item>
                <item>
                    <title>Usability</title>
                    <description>Several test engineers will work with the system, so it should be fairly easy to use and understand, working with the engineers' goals and workflow rather than against them. In addition to human usability, it should also be easy to extend and integrate with current or future autonomous testing workflows.</description>
                </item>
            </criteria>
            <section>
                <title>Getting Physical</title>
                <g>
                    <p>While I can't take credit for the mechanical design or manufacturing of the testing rig's structure, making it move required finding the best possible depth and rotation control system, as well as selecting motors that had the necessary power and precision.</p>
                    <p>For the depth adjustment, we used a winch-like system with a motorized spool lifting the tested device up and down on a telescoping pole, while rotation was achieved by rotating the entire testing rig using a motorized belt. This was done to avoid placing any electronics underwater while maintaining full control over the device's position.</p>
                    <p>For motors, we eventually chose small, high-gear-ratio DC motors (<a href="https://www.dfrobot.com/product-633.html">this one, in particular</a>) with quadrature encoders to provide the necessary torque while remaining power-efficient, small, and inexpensive.</p>
                </g>
            </section>
            <!--
            TODO:
            - PID Loop
            - ESP32 Web Server, API
            - Web App (PicoCSS)
            - "Approaching Absolute Zero (Calibration)"
            -->
        </content>
        <thumbnail>garm_boat.jpg</thumbnail>
        <priority>20</priority>
        </project>
        "#;
        let project: Project = quick_xml::de::from_str(test).unwrap();
        println!("{}", serde_json::to_string_pretty(&project).unwrap());
    }

    fn test_project() -> Project {
        Project {
            name: "Test".to_string(),
            url: "test.html".to_string(),
            description: "a test project".to_string(),
            content: Content {
                sections: vec![Section::Section {
                    title: Some("test section".to_string()),
                    content: vec![Element::Paragraph(Text {
                        text: vec![TextElement::Text("Hello, world!".to_string())],
                    })],
                }],
            },
            thumbnail: "test.png".to_string(),
            skills: Skills {
                skills: vec!["testing".to_string(), "testagain".to_string()],
            },
            priority: 0,
        }
    }
}
